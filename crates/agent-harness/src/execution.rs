use std::sync::Arc;

use agent_core::{
    ActionProposalId, AgentEvent, AgentEventKind, BudgetDimension, LoopEventKind,
    LoopProgressEvent, ModelCallId, ModelRequest, ModelResponse, RunFailureKind, RunOutcome,
    RunStatus, ToolCall, ToolCallId, ToolResult,
};
use tokio::time::{Instant, sleep_until, timeout};

use crate::{
    ActionPreparationError, ActionValidationError, AuditFailurePolicy, AuditPhase, AuditPortError,
    AuditSink, AuthorizationDecision, CapabilityPolicy, CompletedModelInvocation, ExecutionStage,
    HarnessConfig, HarnessError, HarnessOperation, ModelPort, OperationEffect,
    RunCancellationHandle, RunContext, ToolRegistry, ValidatedAction,
    action::{ActionValidator, TextActionDecoder},
};

/// Guarded execution boundary for lifecycle, budget, policy, audit,
/// cancellation, and deadline enforcement.
///
/// Cancellation drops the local adapter future. This is cooperative local
/// cancellation only; it does not prove that a remote side effect stopped.
/// Cancellation is polled before the deadline so simultaneous readiness has
/// deterministic cancellation-first semantics.
///
/// Model/tool/iteration counters are execution reservations, not provider
/// billing metrics. A reservation made before a fail-closed audit error is
/// consumed and is never refunded.
pub struct ExecutionHarness {
    model: Arc<dyn ModelPort>,
    tools: ToolRegistry,
    capability_policy: Arc<dyn CapabilityPolicy>,
    audit: Arc<dyn AuditSink>,
    config: HarnessConfig,
}

impl ExecutionHarness {
    pub fn new(
        model: Arc<dyn ModelPort>,
        tools: ToolRegistry,
        capability_policy: Arc<dyn CapabilityPolicy>,
        audit: Arc<dyn AuditSink>,
        config: HarnessConfig,
    ) -> Self {
        Self {
            model,
            tools,
            capability_policy,
            audit,
            config,
        }
    }

    pub async fn start_run(
        &self,
        context: &mut RunContext,
    ) -> Result<RunCancellationHandle, HarnessError> {
        self.require_status(context, HarnessOperation::StartRun, RequiredStatus::Pending)?;

        let handle = context.commit_running(Instant::now())?;
        let event = context.next_event(AgentEventKind::RunStarted)?;
        match self
            .audit_lifecycle(context, &event, HarnessOperation::StartRun)
            .await
        {
            Ok(()) => Ok(handle),
            Err(error) => {
                if self.config.audit_failure_policy() == AuditFailurePolicy::FailClosed {
                    self.terminalize_after_start_audit_failure(context).await;
                }
                Err(error)
            }
        }
    }

    pub async fn invoke_model(
        &self,
        context: &mut RunContext,
        request: ModelRequest,
    ) -> Result<ModelResponse, HarnessError> {
        self.invoke_model_tracked(context, request)
            .await
            .map(CompletedModelInvocation::into_response)
    }

    pub async fn invoke_model_tracked(
        &self,
        context: &mut RunContext,
        request: ModelRequest,
    ) -> Result<CompletedModelInvocation, HarnessError> {
        self.preflight(context, HarnessOperation::InvokeModel)
            .await?;

        if let Err(error) = context.reserve_model_call() {
            self.terminalize_safety(
                context,
                SafetyTerminalization::BudgetExceeded(error.dimension()),
            )
            .await;
            return Err(error.into());
        }
        let model_call_id = ModelCallId::new();
        let event = context.next_event(AgentEventKind::ModelInvocationStarted {
            model_call_id,
            usage: context.usage().model_calls(),
            limit: context.budget().max_model_calls(),
        })?;
        self.audit_invocation(
            context,
            &event,
            HarnessOperation::InvokeModel,
            AuditPhase::BeforeInvocation,
            OperationEffect::NotInvoked,
        )
        .await?;

        let result = self.invoke_model_port(context, request).await;
        match result {
            Ok(response) => {
                let event = context.next_event(AgentEventKind::ModelInvocationCompleted {
                    model_call_id,
                    token_usage: response.token_usage(),
                })?;
                self.audit_invocation(
                    context,
                    &event,
                    HarnessOperation::InvokeModel,
                    AuditPhase::AfterInvocation,
                    OperationEffect::InvocationStarted,
                )
                .await?;
                Ok(CompletedModelInvocation::new(model_call_id, response))
            }
            Err(HarnessError::ModelPort(error)) => {
                let event =
                    context.next_event(AgentEventKind::ModelInvocationFailed { model_call_id })?;
                self.audit_invocation(
                    context,
                    &event,
                    HarnessOperation::InvokeModel,
                    AuditPhase::AfterInvocation,
                    OperationEffect::InvocationStarted,
                )
                .await?;
                Err(HarnessError::ModelPort(error))
            }
            Err(error) => Err(error),
        }
    }

    pub async fn prepare_action(
        &self,
        context: &mut RunContext,
        invocation: CompletedModelInvocation,
    ) -> Result<ValidatedAction, ActionPreparationError> {
        self.preflight(context, HarnessOperation::PrepareAction)
            .await?;
        let (model_call_id, response) = invocation.into_parts();
        let action_proposal_id = ActionProposalId::new();
        let proposal = match TextActionDecoder::decode(response, action_proposal_id, model_call_id)
        {
            Ok(proposal) => proposal,
            Err(error) => {
                self.record_action_rejected(context, error).await?;
                return Err(error.into());
            }
        };

        let event = context
            .next_event(AgentEventKind::ActionProposed {
                model_call_id,
                action_proposal_id,
                tool_name: proposal.tool_name().clone(),
            })
            .map_err(HarnessError::from)?;
        self.audit_invocation(
            context,
            &event,
            HarnessOperation::PrepareAction,
            AuditPhase::Lifecycle,
            OperationEffect::StateCommitted,
        )
        .await?;

        let validated = match ActionValidator::validate(proposal, &self.tools) {
            Ok(validated) => validated,
            Err(error) => {
                self.record_action_rejected(context, error).await?;
                return Err(error.into());
            }
        };
        let event = context
            .next_event(AgentEventKind::ActionValidated { action_proposal_id })
            .map_err(HarnessError::from)?;
        self.audit_invocation(
            context,
            &event,
            HarnessOperation::PrepareAction,
            AuditPhase::Lifecycle,
            OperationEffect::StateCommitted,
        )
        .await?;
        Ok(validated)
    }

    pub async fn invoke_validated_action(
        &self,
        context: &mut RunContext,
        action: ValidatedAction,
    ) -> Result<ToolResult, HarnessError> {
        self.preflight(context, HarnessOperation::InvokeValidatedAction)
            .await?;
        let action_proposal_id = action.proposal_id();
        let tool_call_id = ToolCallId::new();
        let event = context.next_event(AgentEventKind::ActionExecutionBound {
            action_proposal_id,
            tool_call_id,
        })?;
        self.audit_invocation(
            context,
            &event,
            HarnessOperation::InvokeValidatedAction,
            AuditPhase::BeforeInvocation,
            OperationEffect::NotInvoked,
        )
        .await?;

        self.invoke_tool(context, action.into_tool_call(tool_call_id))
            .await
    }

    pub async fn checkpoint(&self, context: &mut RunContext) -> Result<(), HarnessError> {
        self.preflight(context, HarnessOperation::Checkpoint).await
    }

    pub async fn begin_iteration(&self, context: &mut RunContext) -> Result<u32, HarnessError> {
        self.preflight(context, HarnessOperation::BeginIteration)
            .await?;

        let iteration = match context.reserve_iteration() {
            Ok(iteration) => iteration,
            Err(error) => {
                self.terminalize_safety(
                    context,
                    SafetyTerminalization::BudgetExceeded(error.dimension()),
                )
                .await;
                return Err(error.into());
            }
        };
        let event = context.next_event(AgentEventKind::Loop {
            event: LoopEventKind::IterationStarted {
                iteration,
                usage: context.usage().iterations(),
                limit: context.budget().max_iterations(),
            },
        })?;
        self.audit_invocation(
            context,
            &event,
            HarnessOperation::BeginIteration,
            AuditPhase::Lifecycle,
            OperationEffect::StateCommitted,
        )
        .await?;
        Ok(iteration)
    }

    pub async fn record_loop_progress(
        &self,
        context: &mut RunContext,
        progress: LoopProgressEvent,
    ) -> Result<(), HarnessError> {
        self.preflight(context, HarnessOperation::RecordLoopProgress)
            .await?;

        let event = context.next_event(AgentEventKind::Loop {
            event: progress.into(),
        })?;
        self.audit_invocation(
            context,
            &event,
            HarnessOperation::RecordLoopProgress,
            AuditPhase::Lifecycle,
            OperationEffect::StateCommitted,
        )
        .await
    }

    pub async fn invoke_tool(
        &self,
        context: &mut RunContext,
        call: ToolCall,
    ) -> Result<ToolResult, HarnessError> {
        self.preflight(context, HarnessOperation::InvokeTool)
            .await?;

        let binding = self
            .tools
            .get(call.name())
            .ok_or_else(|| HarnessError::ToolNotFound {
                name: call.name().clone(),
            })?;
        let capability = binding.definition().capability();
        let tool_name = binding.definition().name().clone();

        if let AuthorizationDecision::Denied(denial) = self.capability_policy.authorize(capability)
        {
            let event = context.next_event(AgentEventKind::ToolPolicyDenied {
                tool_call_id: call.id(),
                tool_name,
                capability,
            })?;
            self.audit_invocation(
                context,
                &event,
                HarnessOperation::InvokeTool,
                AuditPhase::BeforeInvocation,
                OperationEffect::NotInvoked,
            )
            .await?;
            return Err(HarnessError::PolicyDenied(denial));
        }

        if let Err(error) = context.reserve_tool_call() {
            self.terminalize_safety(
                context,
                SafetyTerminalization::BudgetExceeded(error.dimension()),
            )
            .await;
            return Err(error.into());
        }
        let tool_call_id = call.id();
        let event = context.next_event(AgentEventKind::ToolInvocationStarted {
            tool_call_id,
            tool_name,
            capability,
            usage: context.usage().tool_calls(),
            limit: context.budget().max_tool_calls(),
        })?;
        self.audit_invocation(
            context,
            &event,
            HarnessOperation::InvokeTool,
            AuditPhase::BeforeInvocation,
            OperationEffect::NotInvoked,
        )
        .await?;

        let port = binding.port();
        let result = self.invoke_tool_port(context, &port, call).await;
        match result {
            Ok(result) => {
                if result.call_id() != tool_call_id {
                    let event = context
                        .next_event(AgentEventKind::ToolInvocationAdapterFailed { tool_call_id })?;
                    self.audit_invocation(
                        context,
                        &event,
                        HarnessOperation::InvokeTool,
                        AuditPhase::AfterInvocation,
                        OperationEffect::InvocationStarted,
                    )
                    .await?;
                    return Err(HarnessError::ToolPort(crate::ToolPortError::AdapterFailure));
                }
                let event_kind = match &result {
                    ToolResult::Succeeded { .. } => {
                        AgentEventKind::ToolInvocationCompleted { tool_call_id }
                    }
                    ToolResult::DomainFailure { failure, .. } => {
                        AgentEventKind::ToolInvocationDomainFailed {
                            tool_call_id,
                            kind: failure.kind(),
                        }
                    }
                };
                let event = context.next_event(event_kind)?;
                self.audit_invocation(
                    context,
                    &event,
                    HarnessOperation::InvokeTool,
                    AuditPhase::AfterInvocation,
                    OperationEffect::InvocationStarted,
                )
                .await?;
                Ok(result)
            }
            Err(HarnessError::ToolPort(error)) => {
                let event = context
                    .next_event(AgentEventKind::ToolInvocationAdapterFailed { tool_call_id })?;
                self.audit_invocation(
                    context,
                    &event,
                    HarnessOperation::InvokeTool,
                    AuditPhase::AfterInvocation,
                    OperationEffect::InvocationStarted,
                )
                .await?;
                Err(HarnessError::ToolPort(error))
            }
            Err(error) => Err(error),
        }
    }

    pub async fn complete_run(&self, context: &mut RunContext) -> Result<(), HarnessError> {
        self.preflight(context, HarnessOperation::CompleteRun)
            .await?;
        context.commit_finished(RunOutcome::Completed)?;
        let event = context.next_event(AgentEventKind::RunFinished {
            outcome: RunOutcome::Completed,
        })?;
        self.audit_lifecycle(context, &event, HarnessOperation::CompleteRun)
            .await
    }

    pub async fn fail_run(
        &self,
        context: &mut RunContext,
        kind: RunFailureKind,
    ) -> Result<(), HarnessError> {
        self.preflight(context, HarnessOperation::FailRun).await?;
        let outcome = RunOutcome::Failed { kind };
        context.commit_finished(outcome.clone())?;
        let event = context.next_event(AgentEventKind::RunFinished { outcome })?;
        self.audit_lifecycle(context, &event, HarnessOperation::FailRun)
            .await
    }

    pub async fn cancel_run(&self, context: &mut RunContext) -> Result<(), HarnessError> {
        self.require_status(
            context,
            HarnessOperation::CancelRun,
            RequiredStatus::Running,
        )?;
        context.request_cancel()?;
        context.commit_finished(RunOutcome::Cancelled)?;
        let event = context.next_event(AgentEventKind::RunFinished {
            outcome: RunOutcome::Cancelled,
        })?;
        self.audit_terminal_best_effort(context, &event).await;
        Ok(())
    }

    async fn invoke_model_port(
        &self,
        context: &mut RunContext,
        request: ModelRequest,
    ) -> Result<ModelResponse, HarnessError> {
        let cancellation = context.cancellation()?;
        let deadline = context.deadline_at()?;
        tokio::select! {
            biased;
            () = cancellation.cancelled() => {
                self.terminalize_safety(context, SafetyTerminalization::Cancelled).await;
                Err(HarnessError::Cancelled { stage: ExecutionStage::Invocation })
            }
            () = sleep_until(deadline) => {
                self.terminalize_safety(context, SafetyTerminalization::DeadlineExceeded).await;
                Err(HarnessError::DeadlineExceeded { stage: ExecutionStage::Invocation })
            }
            result = self.model.invoke(request) => result.map_err(HarnessError::ModelPort),
        }
    }

    async fn invoke_tool_port(
        &self,
        context: &mut RunContext,
        port: &Arc<dyn crate::ToolPort>,
        call: ToolCall,
    ) -> Result<ToolResult, HarnessError> {
        let cancellation = context.cancellation()?;
        let deadline = context.deadline_at()?;
        tokio::select! {
            biased;
            () = cancellation.cancelled() => {
                self.terminalize_safety(context, SafetyTerminalization::Cancelled).await;
                Err(HarnessError::Cancelled { stage: ExecutionStage::Invocation })
            }
            () = sleep_until(deadline) => {
                self.terminalize_safety(context, SafetyTerminalization::DeadlineExceeded).await;
                Err(HarnessError::DeadlineExceeded { stage: ExecutionStage::Invocation })
            }
            result = port.invoke(call) => result.map_err(HarnessError::ToolPort),
        }
    }

    async fn preflight(
        &self,
        context: &mut RunContext,
        operation: HarnessOperation,
    ) -> Result<(), HarnessError> {
        self.require_status(context, operation, RequiredStatus::Running)?;

        let cancellation = context.cancellation()?;
        if cancellation.is_cancelled() {
            self.terminalize_safety(context, SafetyTerminalization::Cancelled)
                .await;
            return Err(HarnessError::Cancelled {
                stage: ExecutionStage::Preflight,
            });
        }

        if Instant::now() >= context.deadline_at()? {
            self.terminalize_safety(context, SafetyTerminalization::DeadlineExceeded)
                .await;
            return Err(HarnessError::DeadlineExceeded {
                stage: ExecutionStage::Preflight,
            });
        }

        Ok(())
    }

    async fn record_action_rejected(
        &self,
        context: &mut RunContext,
        error: ActionValidationError,
    ) -> Result<(), HarnessError> {
        let event = context.next_event(AgentEventKind::ActionRejected {
            model_call_id: error.model_call_id(),
            action_proposal_id: error.proposal_id(),
            reason: error.reason(),
        })?;
        self.audit_invocation(
            context,
            &event,
            HarnessOperation::PrepareAction,
            AuditPhase::Lifecycle,
            OperationEffect::StateCommitted,
        )
        .await
    }

    fn require_status(
        &self,
        context: &RunContext,
        operation: HarnessOperation,
        required: RequiredStatus,
    ) -> Result<(), HarnessError> {
        let valid = matches!(
            (required, context.status()),
            (RequiredStatus::Pending, RunStatus::Pending)
                | (RequiredStatus::Running, RunStatus::Running)
        );
        if valid {
            Ok(())
        } else {
            Err(HarnessError::InvalidLifecycle {
                operation,
                status: context.status().clone(),
            })
        }
    }

    async fn audit_invocation(
        &self,
        context: &mut RunContext,
        event: &AgentEvent,
        operation: HarnessOperation,
        phase: AuditPhase,
        effect: OperationEffect,
    ) -> Result<(), HarnessError> {
        match self.audit_with_run_bounds(context, event).await {
            AuditOutcome::Recorded => Ok(()),
            AuditOutcome::Failed(kind) => {
                self.apply_audit_failure(context, operation, phase, effect, kind)
            }
            AuditOutcome::Cancelled => {
                self.terminalize_safety(context, SafetyTerminalization::Cancelled)
                    .await;
                Err(HarnessError::Cancelled {
                    stage: audit_stage(phase),
                })
            }
            AuditOutcome::DeadlineExceeded => {
                self.terminalize_safety(context, SafetyTerminalization::DeadlineExceeded)
                    .await;
                Err(HarnessError::DeadlineExceeded {
                    stage: audit_stage(phase),
                })
            }
        }
    }

    async fn audit_with_run_bounds(
        &self,
        context: &RunContext,
        event: &AgentEvent,
    ) -> AuditOutcome {
        let Ok(cancellation) = context.cancellation() else {
            return AuditOutcome::Failed(AuditPortError::Unavailable);
        };
        let Ok(deadline) = context.deadline_at() else {
            return AuditOutcome::Failed(AuditPortError::Unavailable);
        };

        tokio::select! {
            biased;
            () = cancellation.cancelled() => AuditOutcome::Cancelled,
            () = sleep_until(deadline) => AuditOutcome::DeadlineExceeded,
            result = timeout(self.config.audit_timeout(), self.audit.record(event)) => {
                match result {
                    Ok(Ok(())) => AuditOutcome::Recorded,
                    Ok(Err(error)) => AuditOutcome::Failed(error),
                    Err(_) => AuditOutcome::Failed(AuditPortError::TimedOut),
                }
            }
        }
    }

    async fn audit_lifecycle(
        &self,
        context: &mut RunContext,
        event: &AgentEvent,
        operation: HarnessOperation,
    ) -> Result<(), HarnessError> {
        match timeout(self.config.audit_timeout(), self.audit.record(event)).await {
            Ok(Ok(())) => Ok(()),
            Ok(Err(kind)) => self.apply_audit_failure(
                context,
                operation,
                AuditPhase::Lifecycle,
                OperationEffect::StateCommitted,
                kind,
            ),
            Err(_) => self.apply_audit_failure(
                context,
                operation,
                AuditPhase::Lifecycle,
                OperationEffect::StateCommitted,
                AuditPortError::TimedOut,
            ),
        }
    }

    fn apply_audit_failure(
        &self,
        context: &mut RunContext,
        operation: HarnessOperation,
        phase: AuditPhase,
        effect: OperationEffect,
        kind: AuditPortError,
    ) -> Result<(), HarnessError> {
        match self.config.audit_failure_policy() {
            AuditFailurePolicy::FailClosed => Err(HarnessError::Audit {
                phase,
                operation,
                effect,
                kind,
            }),
            AuditFailurePolicy::FailOpen => {
                context.mark_audit_degraded();
                Ok(())
            }
        }
    }

    async fn terminalize_safety(&self, context: &mut RunContext, terminal: SafetyTerminalization) {
        if !matches!(context.status(), RunStatus::Running) {
            return;
        }

        let outcome = match terminal {
            SafetyTerminalization::Cancelled => RunOutcome::Cancelled,
            SafetyTerminalization::DeadlineExceeded => RunOutcome::BudgetExceeded {
                dimension: BudgetDimension::Elapsed,
            },
            SafetyTerminalization::BudgetExceeded(dimension) => {
                RunOutcome::BudgetExceeded { dimension }
            }
        };
        if context.commit_finished(outcome.clone()).is_err() {
            context.mark_audit_degraded();
            return;
        }

        match context.next_event(AgentEventKind::RunFinished { outcome }) {
            Ok(event) => self.audit_terminal_best_effort(context, &event).await,
            Err(_) => context.mark_audit_degraded(),
        }
    }

    async fn terminalize_after_start_audit_failure(&self, context: &mut RunContext) {
        if !matches!(context.status(), RunStatus::Running) {
            return;
        }

        let outcome = RunOutcome::Failed {
            kind: RunFailureKind::AuditUnavailable,
        };
        if context.commit_finished(outcome.clone()).is_err() {
            context.mark_audit_degraded();
            return;
        }

        match context.next_event(AgentEventKind::RunFinished { outcome }) {
            Ok(event) => self.audit_terminal_best_effort(context, &event).await,
            Err(_) => context.mark_audit_degraded(),
        }
    }

    async fn audit_terminal_best_effort(&self, context: &mut RunContext, event: &AgentEvent) {
        if !matches!(
            timeout(self.config.audit_timeout(), self.audit.record(event)).await,
            Ok(Ok(()))
        ) {
            context.mark_audit_degraded();
        }
    }
}

fn audit_stage(phase: AuditPhase) -> ExecutionStage {
    match phase {
        AuditPhase::BeforeInvocation => ExecutionStage::PreInvocationAudit,
        AuditPhase::AfterInvocation => ExecutionStage::PostInvocationAudit,
        AuditPhase::Lifecycle => ExecutionStage::LifecycleAudit,
    }
}

#[derive(Clone, Copy)]
enum RequiredStatus {
    Pending,
    Running,
}

enum AuditOutcome {
    Recorded,
    Failed(AuditPortError),
    Cancelled,
    DeadlineExceeded,
}

#[derive(Clone, Copy)]
enum SafetyTerminalization {
    Cancelled,
    DeadlineExceeded,
    BudgetExceeded(BudgetDimension),
}
