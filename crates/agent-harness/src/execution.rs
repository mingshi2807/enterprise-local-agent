use std::{sync::Arc, time::SystemTime};

use agent_core::{
    ActionDigest, ActionProposalId, AgentEvent, AgentEventKind, ApprovalFailureKind,
    ApprovalRequestId, BudgetDimension, CapabilityKind, ContainmentFailureKind,
    DurableApprovalOutcome, DurableApprovalWaitId, EventSequence, GraphFailureKind,
    GraphNodeAttemptId, GraphNodeId, GraphNodeKind, GraphProgressEvent, GraphRecoveryMode,
    GraphTransitionKey, KnowledgeFailureKind, KnowledgeRetrievalId, LoopEventKind,
    LoopProgressEvent, ModelCallId, ModelRequest, ModelResponse, RunFailureKind, RunOutcome,
    RunStatus, ToolCall, ToolCallId, ToolName, ToolResult, WorkspaceBindingId,
};
use agent_knowledge::{
    EvidenceSet, GroundedModelRequest, KnowledgeError, KnowledgePort, KnowledgeRequest,
};
use serde_json::{Value, json};
use tokio::time::{Instant, sleep_until, timeout};

use crate::{
    ActionPreparationError, ActionSealBinding, ActionSealPort, ActionValidationError,
    ApprovalOutcome, ApprovalPort, ApprovalPortError, ApprovalRequest, AuditFailurePolicy,
    AuditPhase, AuditPortError, AuditSink, AuthorizationDecision, CapabilityPolicy,
    CompletedModelInvocation, ContainedToolPort, CreateDurableApprovalWait, DurableActionContext,
    DurableApprovalDecisionCommand, DurableApprovalRecord, DurableApprovalStatus,
    DurableApprovalStatusTransition, DurableApprovalView, DurableApprovalWait,
    DurableGraphPosition, DurableLocalWriteResume, ExecutionStage, HarnessConfig, HarnessError,
    HarnessOperation, ModelPort, OperationEffect, PersistencePortError, PolicyDenial,
    PolicyDenialReason, RecordDurableApprovalDecision, RecoveredWaitingRun, RecoveryContract,
    RecoveryDisposition, RunCancellationHandle, RunContext, RunKey, RunPersistencePort, ToolPort,
    ToolRegistry, ValidatedAction,
    action::{
        ActionValidator, TextActionDecoder, compute_action_digest, compute_tool_contract_digest,
        validate_argument_limits,
    },
    registry::ExecutionBinding,
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
    approval: Option<Arc<dyn ApprovalPort>>,
    audit: Arc<dyn AuditSink>,
    persistence: Option<Arc<dyn RunPersistencePort>>,
    knowledge: Option<Arc<dyn KnowledgePort>>,
    action_seal: Option<Arc<dyn ActionSealPort>>,
    workspace_binding_id: Option<WorkspaceBindingId>,
    config: HarnessConfig,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CompletedKnowledgeRetrieval {
    retrieval_id: KnowledgeRetrievalId,
    evidence: EvidenceSet,
}

impl CompletedKnowledgeRetrieval {
    #[must_use]
    pub const fn retrieval_id(&self) -> KnowledgeRetrievalId {
        self.retrieval_id
    }

    #[must_use]
    pub const fn evidence(&self) -> &EvidenceSet {
        &self.evidence
    }
}

#[derive(Clone, Copy)]
struct GroundingBinding {
    retrieval_id: KnowledgeRetrievalId,
    evidence_count: u8,
    manifest_digest: [u8; 32],
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
            approval: None,
            audit,
            persistence: None,
            knowledge: None,
            action_seal: None,
            workspace_binding_id: None,
            config,
        }
    }

    #[must_use]
    pub fn with_approval_port(mut self, approval: Arc<dyn ApprovalPort>) -> Self {
        self.approval = Some(approval);
        self
    }

    #[must_use]
    pub fn with_persistence_port(mut self, persistence: Arc<dyn RunPersistencePort>) -> Self {
        self.persistence = Some(persistence);
        self
    }

    #[must_use]
    pub fn with_knowledge_port(mut self, knowledge: Arc<dyn KnowledgePort>) -> Self {
        self.knowledge = Some(knowledge);
        self
    }

    #[must_use]
    pub fn with_durable_local_write_approval(
        mut self,
        action_seal: Arc<dyn ActionSealPort>,
        workspace_binding_id: WorkspaceBindingId,
    ) -> Self {
        self.action_seal = Some(action_seal);
        self.workspace_binding_id = Some(workspace_binding_id);
        self
    }

    pub async fn recover_run(&self, key: RunKey) -> Result<RecoveryDisposition, HarnessError> {
        let persistence = self
            .persistence
            .as_ref()
            .ok_or(HarnessError::Persistence(PersistencePortError::Unavailable))?;
        let loaded = persistence
            .load_run(key)
            .await
            .map_err(HarnessError::Persistence)?;
        let now_wall = SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|_| HarnessError::Recovery(crate::RecoveryError::ClockAmbiguous))?
            .as_millis()
            .try_into()
            .map_err(|_| HarnessError::Recovery(crate::RecoveryError::ClockAmbiguous))?;
        crate::recovery::recover_loaded_run(loaded, now_wall, Instant::now())
            .map_err(HarnessError::Recovery)
    }

    pub async fn start_run(
        &self,
        context: &mut RunContext,
    ) -> Result<RunCancellationHandle, HarnessError> {
        self.require_status(context, HarnessOperation::StartRun, RequiredStatus::Pending)?;

        if let Some(persistence) = &self.persistence {
            persistence
                .create_run(&context.run_record(), &context.durable_checkpoint())
                .await
                .map_err(HarnessError::Persistence)?;
        }

        let handle = context.commit_running(Instant::now(), SystemTime::now())?;
        let event = context.next_event(AgentEventKind::RunStarted {
            started_at_unix_millis: context.started_at_unix_millis()?,
        })?;
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
        self.invoke_model_internal(context, request, None).await
    }

    pub async fn invoke_grounded_model(
        &self,
        context: &mut RunContext,
        retrieval: &CompletedKnowledgeRetrieval,
        request: GroundedModelRequest,
    ) -> Result<CompletedModelInvocation, HarnessError> {
        if request.manifest_digest() != retrieval.evidence.manifest_digest()
            || request.evidence_count()
                != u8::try_from(retrieval.evidence.evidence().len()).unwrap_or(u8::MAX)
        {
            return Err(HarnessError::GroundingMismatch);
        }
        let binding = GroundingBinding {
            retrieval_id: retrieval.retrieval_id,
            evidence_count: request.evidence_count(),
            manifest_digest: request.manifest_digest(),
        };
        self.invoke_model_internal(context, request.into_request(), Some(binding))
            .await
    }

    async fn invoke_model_internal(
        &self,
        context: &mut RunContext,
        request: ModelRequest,
        grounding: Option<GroundingBinding>,
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
        if let Some(grounding) = grounding {
            let event = context.next_event(AgentEventKind::ModelGroundingBound {
                retrieval_id: grounding.retrieval_id,
                model_call_id,
                evidence_count: grounding.evidence_count,
                manifest_digest: grounding.manifest_digest,
            })?;
            self.audit_invocation(
                context,
                &event,
                HarnessOperation::BindModelGrounding,
                AuditPhase::Lifecycle,
                OperationEffect::NotInvoked,
            )
            .await?;
        }

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

    pub async fn retrieve_knowledge(
        &self,
        context: &mut RunContext,
        request: KnowledgeRequest,
    ) -> Result<CompletedKnowledgeRetrieval, HarnessError> {
        self.preflight(context, HarnessOperation::RetrieveKnowledge)
            .await?;
        let knowledge = self.knowledge.as_ref().ok_or(HarnessError::KnowledgePort(
            KnowledgeError::Unavailable(request.route().backends()[0]),
        ))?;
        let retrieval_id = KnowledgeRetrievalId::new();
        let route = request.route().metadata();
        let query_digest = request.query().digest();
        let query_bytes = u16::try_from(request.query().as_str().len())
            .map_err(|_| HarnessError::KnowledgePort(KnowledgeError::MalformedResponse))?;
        let event_kind = match context.durable_state().pending_knowledge_retrieval() {
            Some(previous) => {
                if previous.route() != &route
                    || previous.query_digest() != query_digest
                    || previous.query_bytes() != query_bytes
                {
                    return Err(HarnessError::KnowledgePort(KnowledgeError::Rejected(
                        request.route().backends()[0],
                    )));
                }
                AgentEventKind::KnowledgeRetrievalRestarted {
                    previous_retrieval_id: previous.retrieval_id(),
                    retrieval_id,
                    route,
                    query_digest,
                    query_bytes,
                }
            }
            None => AgentEventKind::KnowledgeRetrievalStarted {
                retrieval_id,
                route,
                query_digest,
                query_bytes,
            },
        };
        let event = context.next_event(event_kind)?;
        self.audit_invocation(
            context,
            &event,
            HarnessOperation::RetrieveKnowledge,
            AuditPhase::BeforeInvocation,
            OperationEffect::NotInvoked,
        )
        .await?;

        let cancellation = context.cancellation()?;
        let deadline = context.deadline_at()?;
        let outcome = tokio::select! {
            biased;
            () = cancellation.cancelled() => KnowledgeWaitOutcome::Cancelled,
            () = sleep_until(deadline) => KnowledgeWaitOutcome::DeadlineExceeded,
            result = knowledge.retrieve(request) => KnowledgeWaitOutcome::Result(result),
        };
        match outcome {
            KnowledgeWaitOutcome::Result(Ok(evidence)) => {
                let references = evidence
                    .evidence()
                    .iter()
                    .map(agent_knowledge::Evidence::durable_reference)
                    .collect::<Option<Vec<_>>>()
                    .ok_or(HarnessError::KnowledgePort(
                        KnowledgeError::MalformedResponse,
                    ))?;
                let evidence_count = u8::try_from(references.len())
                    .map_err(|_| HarnessError::KnowledgePort(KnowledgeError::MalformedResponse))?;
                let event = context.next_event(AgentEventKind::KnowledgeRetrievalCompleted {
                    retrieval_id,
                    snapshots: evidence.snapshots().to_vec(),
                    evidence_references: references,
                    evidence_count,
                    truncated: evidence.truncated(),
                    degraded: evidence.degraded(),
                    manifest_digest: evidence.manifest_digest(),
                })?;
                self.audit_invocation(
                    context,
                    &event,
                    HarnessOperation::RetrieveKnowledge,
                    AuditPhase::AfterInvocation,
                    OperationEffect::InvocationStarted,
                )
                .await?;
                Ok(CompletedKnowledgeRetrieval {
                    retrieval_id,
                    evidence,
                })
            }
            KnowledgeWaitOutcome::Result(Err(error)) => {
                self.record_knowledge_failure(context, retrieval_id, knowledge_failure_kind(error))
                    .await?;
                Err(HarnessError::KnowledgePort(error))
            }
            KnowledgeWaitOutcome::Cancelled => {
                let result = self
                    .record_knowledge_interruption(
                        context,
                        retrieval_id,
                        KnowledgeFailureKind::Cancelled,
                    )
                    .await;
                result?;
                self.terminalize_safety(context, SafetyTerminalization::Cancelled)
                    .await;
                Err(HarnessError::Cancelled {
                    stage: ExecutionStage::Invocation,
                })
            }
            KnowledgeWaitOutcome::DeadlineExceeded => {
                let result = self
                    .record_knowledge_interruption(
                        context,
                        retrieval_id,
                        KnowledgeFailureKind::DeadlineExceeded,
                    )
                    .await;
                result?;
                self.terminalize_safety(context, SafetyTerminalization::DeadlineExceeded)
                    .await;
                Err(HarnessError::DeadlineExceeded {
                    stage: ExecutionStage::Invocation,
                })
            }
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
        let tool_name = action.tool_name().clone();
        let capability = action.capability();
        let action_digest = action.digest();
        let binding =
            self.tools
                .get(action.tool_name())
                .ok_or_else(|| HarnessError::ToolNotFound {
                    name: action.tool_name().clone(),
                })?;
        if binding.definition().capability() != capability {
            return Err(HarnessError::ToolPort(crate::ToolPortError::AdapterFailure));
        }
        let event = context.next_event(AgentEventKind::ActionExecutionBound {
            action_proposal_id,
            tool_call_id,
        })?;
        if capability == CapabilityKind::LocalWrite {
            self.audit_required(
                context,
                &event,
                HarnessOperation::InvokeValidatedAction,
                AuditPhase::BeforeInvocation,
                OperationEffect::NotInvoked,
            )
            .await?;
        } else {
            self.audit_invocation(
                context,
                &event,
                HarnessOperation::InvokeValidatedAction,
                AuditPhase::BeforeInvocation,
                OperationEffect::NotInvoked,
            )
            .await?;
        }

        let decision = self.capability_policy.authorize(capability);
        match capability {
            CapabilityKind::ReadOnly => match decision {
                AuthorizationDecision::Allowed => match binding.execution().clone() {
                    ExecutionBinding::Direct(port) => {
                        self.invoke_direct_tool(
                            context,
                            port,
                            action.into_tool_call(tool_call_id),
                            tool_name,
                            capability,
                        )
                        .await
                    }
                    ExecutionBinding::ManagedDirect(port) => {
                        self.invoke_managed_tool(
                            context,
                            port,
                            action.into_tool_call(tool_call_id),
                            tool_name,
                            capability,
                        )
                        .await
                    }
                    ExecutionBinding::Contained(port) => {
                        self.invoke_contained_read_only(
                            context,
                            port,
                            action.into_tool_call(tool_call_id),
                            tool_name,
                            capability,
                        )
                        .await
                    }
                },
                AuthorizationDecision::RequiresApproval => {
                    self.record_policy_denied(
                        context,
                        tool_call_id,
                        tool_name,
                        capability,
                        HarnessOperation::InvokeValidatedAction,
                    )
                    .await?;
                    Err(HarnessError::PolicyDenied(PolicyDenial::new(
                        capability,
                        PolicyDenialReason::ApprovalRequiredButUnsupported,
                    )))
                }
                AuthorizationDecision::Denied(denial) => {
                    self.record_policy_denied(
                        context,
                        tool_call_id,
                        tool_name,
                        capability,
                        HarnessOperation::InvokeValidatedAction,
                    )
                    .await?;
                    Err(HarnessError::PolicyDenied(denial))
                }
            },
            CapabilityKind::LocalWrite => match decision {
                AuthorizationDecision::Denied(denial) => {
                    self.record_policy_denied(
                        context,
                        tool_call_id,
                        tool_name,
                        capability,
                        HarnessOperation::InvokeValidatedAction,
                    )
                    .await?;
                    Err(HarnessError::PolicyDenied(denial))
                }
                AuthorizationDecision::Allowed => {
                    self.record_policy_denied(
                        context,
                        tool_call_id,
                        tool_name,
                        capability,
                        HarnessOperation::InvokeValidatedAction,
                    )
                    .await?;
                    Err(HarnessError::ApprovalRequired)
                }
                AuthorizationDecision::RequiresApproval => match binding.execution().clone() {
                    ExecutionBinding::Direct(_) => {
                        self.record_containment_failed(
                            context,
                            tool_call_id,
                            tool_name.clone(),
                            capability,
                            ContainmentFailureKind::Unavailable,
                        )
                        .await?;
                        Err(HarnessError::ContainmentUnavailable { name: tool_name })
                    }
                    ExecutionBinding::ManagedDirect(_) => {
                        self.record_containment_failed(
                            context,
                            tool_call_id,
                            tool_name.clone(),
                            capability,
                            ContainmentFailureKind::Unavailable,
                        )
                        .await?;
                        Err(HarnessError::ContainmentUnavailable { name: tool_name })
                    }
                    ExecutionBinding::Contained(port) => {
                        if context.audit_degraded() {
                            return Err(HarnessError::AuditDegraded);
                        }
                        self.invoke_approved_local_write(
                            context,
                            port,
                            action.into_tool_call(tool_call_id),
                            ApprovedLocalWriteMeta {
                                action_proposal_id,
                                tool_name,
                                capability,
                                action_digest,
                            },
                        )
                        .await
                    }
                },
            },
            CapabilityKind::ExternalWrite | CapabilityKind::Privileged => {
                self.record_policy_denied(
                    context,
                    tool_call_id,
                    tool_name,
                    capability,
                    HarnessOperation::InvokeValidatedAction,
                )
                .await?;
                Err(HarnessError::CapabilityNotExecutable { capability })
            }
        }
    }

    pub async fn suspend_durable_local_write(
        &self,
        context: &mut RunContext,
        action: ValidatedAction,
        durable: DurableActionContext,
    ) -> Result<DurableApprovalWait, HarnessError> {
        self.preflight(context, HarnessOperation::RequestApproval)
            .await?;
        let persistence = self
            .persistence
            .as_ref()
            .ok_or(HarnessError::Persistence(PersistencePortError::Unavailable))?;
        let action_seal = self.action_seal.as_ref().ok_or(HarnessError::ActionSeal(
            crate::ActionSealError::KeyUnavailable,
        ))?;
        let workspace_binding_id = self
            .workspace_binding_id
            .ok_or(HarnessError::DurableApprovalMismatch)?;
        if context.recovery_contract()
            != (RecoveryContract::Graph {
                program_version: durable.program_version,
                definition_digest: durable.graph_digest.as_bytes(),
            })
            || !matches!(
                context.durable_state().graph().map(crate::DurableGraphState::position),
                Some(DurableGraphPosition::Running {
                    attempt_id,
                    node_id,
                    node_kind: GraphNodeKind::Action,
                    recovery: GraphRecoveryMode::Never,
                }) if *attempt_id == durable.attempt_id && node_id == &durable.node_id
            )
        {
            return Err(HarnessError::DurableApprovalMismatch);
        }

        let (action_proposal_id, tool_name, arguments, capability, action_digest) =
            action.into_durable_parts();
        if tool_name.as_str() != DURABLE_LOCAL_WRITE_TOOL_NAME
            || capability != CapabilityKind::LocalWrite
            || self.capability_policy.authorize(capability)
                != AuthorizationDecision::RequiresApproval
        {
            return Err(HarnessError::DurableApprovalMismatch);
        }
        let binding = self
            .tools
            .get(&tool_name)
            .ok_or_else(|| HarnessError::ToolNotFound {
                name: tool_name.clone(),
            })?;
        let ExecutionBinding::Contained(port) = binding.execution().clone() else {
            return Err(HarnessError::ContainmentUnavailable { name: tool_name });
        };
        if context.audit_degraded() {
            return Err(HarnessError::AuditDegraded);
        }
        let preview = port
            .approval_preview(&arguments)
            .map_err(HarnessError::ContainmentPort)?;
        let capsule = local_write_capsule(arguments.as_value())?;
        let tool_contract_digest = compute_tool_contract_digest(binding.definition());
        let tool_call_id = ToolCallId::new();
        let approval_request_id = ApprovalRequestId::new();
        let wait_id = DurableApprovalWaitId::new();

        let event = context.next_event(AgentEventKind::ActionExecutionBound {
            action_proposal_id,
            tool_call_id,
        })?;
        self.audit_required(
            context,
            &event,
            HarnessOperation::InvokeValidatedAction,
            AuditPhase::BeforeInvocation,
            OperationEffect::NotInvoked,
        )
        .await?;
        if let Err(error) = context.reserve_approval_request() {
            self.terminalize_safety(
                context,
                SafetyTerminalization::BudgetExceeded(error.dimension()),
            )
            .await;
            return Err(error.into());
        }
        let seal_binding = ActionSealBinding::new(
            RunKey::new(context.run_id(), context.session_id()),
            durable.program_version,
            durable.graph_digest,
            durable.node_id.clone(),
            durable.attempt_id,
            wait_id,
            approval_request_id,
            action_proposal_id,
            tool_call_id,
            action_digest,
            workspace_binding_id,
            tool_contract_digest,
        );
        let event = context.next_event(AgentEventKind::DurableApprovalPrepared {
            wait_id,
            approval_request_id,
            action_proposal_id,
            tool_call_id,
            action_digest,
            workspace_binding_id,
            tool_contract_digest,
            usage: context.usage().approval_requests(),
            limit: context.budget().max_approval_requests(),
        })?;
        self.audit_required(
            context,
            &event,
            HarnessOperation::RequestApproval,
            AuditPhase::BeforeInvocation,
            OperationEffect::NotInvoked,
        )
        .await?;

        let sealed = action_seal.seal_local_write(&seal_binding, &capsule)?;
        let event = context.next_event(AgentEventKind::Graph {
            event: GraphProgressEvent::GraphSuspended {
                attempt_id: durable.attempt_id,
                node_id: durable.node_id,
                wait_id,
                approval_request_id,
                action_proposal_id,
                tool_call_id,
                action_digest,
            },
        })?;
        let transition = self.transition_for_event(context, &event, true);
        let wait = DurableApprovalWait::new(&seal_binding);
        if let Err(error) = persistence
            .create_durable_approval_wait(&CreateDurableApprovalWait::new(
                transition,
                DurableApprovalRecord::new(seal_binding, sealed),
            ))
            .await
        {
            context.mark_persistence_failed();
            return Err(HarnessError::Persistence(error));
        }
        self.audit_already_persisted(
            context,
            &event,
            HarnessOperation::RequestApproval,
            AuditPhase::Lifecycle,
            OperationEffect::StateCommitted,
            false,
        )
        .await?;
        let _ = preview;
        Ok(wait)
    }

    pub async fn durable_approval_view(
        &self,
        key: RunKey,
        wait_id: DurableApprovalWaitId,
    ) -> Result<DurableApprovalView, HarnessError> {
        let recovered = match self.recover_run(key).await? {
            RecoveryDisposition::Waiting(recovered) => recovered,
            _ => return Err(HarnessError::DurableApprovalMismatch),
        };
        let persistence = self
            .persistence
            .as_ref()
            .ok_or(HarnessError::Persistence(PersistencePortError::Unavailable))?;
        let record = persistence
            .load_durable_approval_wait(key, wait_id)
            .await
            .map_err(HarnessError::Persistence)?;
        let (_, port, preview) = self.restore_local_write(&record, recovered.state())?;
        let _ = port;
        Ok(DurableApprovalView::new(
            wait_id,
            record.binding().approval_request_id(),
            record.row_version(),
            preview,
        ))
    }

    pub async fn record_durable_approval_decision(
        &self,
        command: DurableApprovalDecisionCommand,
    ) -> Result<(), HarnessError> {
        let recovered = match self.recover_run(command.key).await? {
            RecoveryDisposition::Waiting(recovered) => recovered,
            _ => return Err(HarnessError::DurableApprovalMismatch),
        };
        let (mut context, state, _) = recovered.into_parts();
        let waiting = waiting_position(&state, command.wait_id)?;
        if waiting.1 != command.approval_request_id
            || waiting.2 != command.action_proposal_id
            || waiting.3 != command.tool_call_id
            || waiting.4 != command.action_digest
        {
            return Err(HarnessError::DurableApprovalMismatch);
        }
        let next_version = command
            .expected_row_version
            .checked_add(1)
            .ok_or(HarnessError::DurableApprovalMismatch)?;
        let event = context.next_event(AgentEventKind::DurableApprovalDecisionRecorded {
            wait_id: command.wait_id,
            approval_request_id: command.approval_request_id,
            outcome: command.outcome,
            row_version: next_version,
        })?;
        let transition = self.transition_for_event(&context, &event, true);
        if let Err(error) = self
            .persistence
            .as_ref()
            .ok_or(HarnessError::Persistence(PersistencePortError::Unavailable))?
            .record_durable_approval_decision(&RecordDurableApprovalDecision::new(
                transition, command,
            ))
            .await
        {
            context.mark_persistence_failed();
            return Err(HarnessError::Persistence(error));
        }
        self.audit_already_persisted(
            &mut context,
            &event,
            HarnessOperation::RequestApproval,
            AuditPhase::AfterInvocation,
            OperationEffect::NotInvoked,
            false,
        )
        .await
    }

    pub async fn record_durable_approval_outcome(
        &self,
        key: RunKey,
        wait_id: DurableApprovalWaitId,
        expected_row_version: u64,
        outcome: DurableApprovalOutcome,
    ) -> Result<(), HarnessError> {
        let recovered = match self.recover_run(key).await? {
            RecoveryDisposition::Waiting(recovered) => recovered,
            _ => return Err(HarnessError::DurableApprovalMismatch),
        };
        let waiting = waiting_position(recovered.state(), wait_id)?;
        self.record_durable_approval_decision(DurableApprovalDecisionCommand {
            key,
            wait_id,
            approval_request_id: waiting.1,
            action_proposal_id: waiting.2,
            tool_call_id: waiting.3,
            action_digest: waiting.4,
            expected_row_version,
            outcome,
        })
        .await
    }

    pub async fn abort_durable_waiting(
        &self,
        key: RunKey,
        wait_id: DurableApprovalWaitId,
        expected_row_version: u64,
    ) -> Result<(), HarnessError> {
        let recovered = match self.recover_run(key).await? {
            RecoveryDisposition::Waiting(recovered) => recovered,
            _ => return Err(HarnessError::DurableApprovalMismatch),
        };
        let (mut context, state, _) = recovered.into_parts();
        let _ = waiting_position(&state, wait_id)?;
        let persistence = self
            .persistence
            .as_ref()
            .ok_or(HarnessError::Persistence(PersistencePortError::Unavailable))?;
        let record = persistence
            .load_durable_approval_wait(key, wait_id)
            .await
            .map_err(HarnessError::Persistence)?;
        if record.status() != DurableApprovalStatus::Waiting
            || record.row_version() != expected_row_version
        {
            return Err(HarnessError::DurableApprovalMismatch);
        }
        context.commit_finished(RunOutcome::Cancelled)?;
        let event = context.next_event(AgentEventKind::RunFinished {
            outcome: RunOutcome::Cancelled,
        })?;
        let transition = self.transition_for_event(&context, &event, true);
        persistence
            .transition_durable_approval_status(&DurableApprovalStatusTransition::new(
                Some(transition),
                key,
                wait_id,
                expected_row_version,
                DurableApprovalStatus::Waiting,
                DurableApprovalStatus::Consumed,
            ))
            .await
            .map_err(HarnessError::Persistence)?;
        self.audit_already_persisted(
            &mut context,
            &event,
            HarnessOperation::CancelRun,
            AuditPhase::Lifecycle,
            OperationEffect::StateCommitted,
            false,
        )
        .await
    }

    pub async fn resume_durable_local_write(
        &self,
        recovered: RecoveredWaitingRun,
        wait_id: DurableApprovalWaitId,
    ) -> Result<DurableLocalWriteResume, HarnessError> {
        let (mut context, state, _) = recovered.into_parts();
        let (attempt_id, approval_request_id, _, _, _) = waiting_position(&state, wait_id)?;
        let node_id = match state.graph().map(crate::DurableGraphState::position) {
            Some(DurableGraphPosition::Waiting { node_id, .. }) => node_id.clone(),
            _ => return Err(HarnessError::DurableApprovalMismatch),
        };
        let persistence = self
            .persistence
            .as_ref()
            .ok_or(HarnessError::Persistence(PersistencePortError::Unavailable))?;
        let mut record = persistence
            .load_durable_approval_wait(
                RunKey::new(context.run_id(), context.session_id()),
                wait_id,
            )
            .await
            .map_err(HarnessError::Persistence)?;

        if record.status() == DurableApprovalStatus::DecisionRecordedDeny {
            let event = context.next_event(AgentEventKind::DurableApprovalDenied {
                wait_id,
                approval_request_id,
            })?;
            self.audit_required(
                &mut context,
                &event,
                HarnessOperation::RequestApproval,
                AuditPhase::AfterInvocation,
                OperationEffect::NotInvoked,
            )
            .await?;
            record = persistence
                .load_durable_approval_wait(record.binding().key(), wait_id)
                .await
                .map_err(HarnessError::Persistence)?;
            let resume = context.next_event(AgentEventKind::Graph {
                event: GraphProgressEvent::GraphResumed {
                    attempt_id,
                    node_id: node_id.clone(),
                    wait_id,
                },
            })?;
            if let Err(error) = persistence
                .transition_durable_approval_status(&DurableApprovalStatusTransition::new(
                    Some(self.transition_for_event(&context, &resume, false)),
                    record.binding().key(),
                    wait_id,
                    record.row_version(),
                    DurableApprovalStatus::DecisionRecordedDeny,
                    DurableApprovalStatus::Consumed,
                ))
                .await
            {
                context.mark_persistence_failed();
                return Err(HarnessError::Persistence(error));
            }
            self.audit_already_persisted(
                &mut context,
                &resume,
                HarnessOperation::RequestApproval,
                AuditPhase::Lifecycle,
                OperationEffect::StateCommitted,
                false,
            )
            .await?;
            return Ok(DurableLocalWriteResume::Denied {
                context,
                attempt_id,
                node_id,
            });
        }
        if !matches!(
            record.status(),
            DurableApprovalStatus::DecisionRecordedApprove | DurableApprovalStatus::ApprovedReady
        ) {
            return Err(HarnessError::DurableApprovalMismatch);
        }

        let (action, port, _) = self.restore_local_write(&record, &state)?;
        if record.status() == DurableApprovalStatus::DecisionRecordedApprove {
            let event = context.next_event(AgentEventKind::DurableApprovalGranted {
                wait_id,
                approval_request_id,
            })?;
            self.audit_required(
                &mut context,
                &event,
                HarnessOperation::RequestApproval,
                AuditPhase::AfterInvocation,
                OperationEffect::NotInvoked,
            )
            .await?;
            if let Err(error) = persistence
                .transition_durable_approval_status(&DurableApprovalStatusTransition::new(
                    None,
                    record.binding().key(),
                    wait_id,
                    record.row_version(),
                    DurableApprovalStatus::DecisionRecordedApprove,
                    DurableApprovalStatus::ApprovedReady,
                ))
                .await
            {
                context.mark_persistence_failed();
                return Err(HarnessError::Persistence(error));
            }
            record = persistence
                .load_durable_approval_wait(record.binding().key(), wait_id)
                .await
                .map_err(HarnessError::Persistence)?;
        }
        let resume = context.next_event(AgentEventKind::Graph {
            event: GraphProgressEvent::GraphResumed {
                attempt_id,
                node_id: node_id.clone(),
                wait_id,
            },
        })?;
        if let Err(error) = persistence
            .transition_durable_approval_status(&DurableApprovalStatusTransition::new(
                Some(self.transition_for_event(&context, &resume, false)),
                record.binding().key(),
                wait_id,
                record.row_version(),
                DurableApprovalStatus::ApprovedReady,
                DurableApprovalStatus::ApprovedReady,
            ))
            .await
        {
            context.mark_persistence_failed();
            return Err(HarnessError::Persistence(error));
        }
        self.audit_already_persisted(
            &mut context,
            &resume,
            HarnessOperation::RequestApproval,
            AuditPhase::Lifecycle,
            OperationEffect::StateCommitted,
            false,
        )
        .await?;
        record = persistence
            .load_durable_approval_wait(record.binding().key(), wait_id)
            .await
            .map_err(HarnessError::Persistence)?;

        self.preflight(&mut context, HarnessOperation::InvokeTool)
            .await?;
        if let Err(error) = context.reserve_tool_call() {
            self.terminalize_safety(
                &mut context,
                SafetyTerminalization::BudgetExceeded(error.dimension()),
            )
            .await;
            return Err(error.into());
        }
        let event = context.next_event(AgentEventKind::ToolInvocationStarted {
            tool_call_id: record.binding().tool_call_id(),
            tool_name: action.tool_name().clone(),
            capability: action.capability(),
            usage: context.usage().tool_calls(),
            limit: context.budget().max_tool_calls(),
        })?;
        if let Err(error) = persistence
            .transition_durable_approval_status(&DurableApprovalStatusTransition::new(
                Some(self.transition_for_event(&context, &event, false)),
                record.binding().key(),
                wait_id,
                record.row_version(),
                DurableApprovalStatus::ApprovedReady,
                DurableApprovalStatus::Executing,
            ))
            .await
        {
            context.mark_persistence_failed();
            return Err(HarnessError::Persistence(error));
        }
        self.audit_already_persisted(
            &mut context,
            &event,
            HarnessOperation::InvokeTool,
            AuditPhase::BeforeInvocation,
            OperationEffect::NotInvoked,
            true,
        )
        .await?;
        let tool_call_id = record.binding().tool_call_id();
        let result = self
            .invoke_contained_tool_port(&mut context, &port, action.into_tool_call(tool_call_id))
            .await;
        let result = self
            .finish_tool_invocation(
                &mut context,
                tool_call_id,
                HarnessOperation::InvokeTool,
                result,
                ToolTerminalAudit::Required,
            )
            .await?;
        record = persistence
            .load_durable_approval_wait(record.binding().key(), wait_id)
            .await
            .map_err(HarnessError::Persistence)?;
        if let Err(error) = persistence
            .transition_durable_approval_status(&DurableApprovalStatusTransition::new(
                None,
                record.binding().key(),
                wait_id,
                record.row_version(),
                DurableApprovalStatus::Executing,
                DurableApprovalStatus::Consumed,
            ))
            .await
        {
            context.mark_persistence_failed();
            return Err(HarnessError::Persistence(error));
        }
        Ok(DurableLocalWriteResume::Executed {
            context,
            attempt_id,
            node_id,
            result,
        })
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

    pub async fn start_graph(
        &self,
        context: &mut RunContext,
        definition_digest: [u8; 32],
        start_node: GraphNodeId,
        start_kind: GraphNodeKind,
        start_recovery: GraphRecoveryMode,
    ) -> Result<(), HarnessError> {
        self.preflight(context, HarnessOperation::StartGraph)
            .await?;
        let event = context.next_event(AgentEventKind::Graph {
            event: GraphProgressEvent::GraphStarted {
                definition_digest,
                start_node,
                start_kind,
                start_recovery,
            },
        })?;
        self.audit_invocation(
            context,
            &event,
            HarnessOperation::StartGraph,
            AuditPhase::Lifecycle,
            OperationEffect::StateCommitted,
        )
        .await
    }

    pub async fn enter_graph_node(
        &self,
        context: &mut RunContext,
        node_id: GraphNodeId,
        node_kind: GraphNodeKind,
        recovery: GraphRecoveryMode,
    ) -> Result<GraphNodeAttemptId, HarnessError> {
        self.preflight(context, HarnessOperation::EnterGraphNode)
            .await?;
        let step = match context.reserve_graph_step() {
            Ok(step) => step,
            Err(error) => {
                self.terminalize_safety(
                    context,
                    SafetyTerminalization::BudgetExceeded(error.dimension()),
                )
                .await;
                return Err(error.into());
            }
        };
        let attempt_id = GraphNodeAttemptId::new();
        let event = context.next_event(AgentEventKind::Graph {
            event: GraphProgressEvent::GraphNodeEntered {
                attempt_id,
                node_id,
                node_kind,
                recovery,
                step,
                limit: context.budget().max_graph_steps(),
            },
        })?;
        self.audit_invocation(
            context,
            &event,
            HarnessOperation::EnterGraphNode,
            AuditPhase::Lifecycle,
            OperationEffect::StateCommitted,
        )
        .await?;
        Ok(attempt_id)
    }

    pub async fn restart_graph_node(
        &self,
        context: &mut RunContext,
        previous_attempt_id: Option<GraphNodeAttemptId>,
        node_id: GraphNodeId,
        node_kind: GraphNodeKind,
        recovery: GraphRecoveryMode,
    ) -> Result<GraphNodeAttemptId, HarnessError> {
        self.preflight(context, HarnessOperation::EnterGraphNode)
            .await?;
        let step = match context.reserve_graph_step() {
            Ok(step) => step,
            Err(error) => {
                self.terminalize_safety(
                    context,
                    SafetyTerminalization::BudgetExceeded(error.dimension()),
                )
                .await;
                return Err(error.into());
            }
        };
        let attempt_id = GraphNodeAttemptId::new();
        let event = context.next_event(AgentEventKind::Graph {
            event: GraphProgressEvent::GraphNodeRestarted {
                previous_attempt_id,
                attempt_id,
                node_id,
                node_kind,
                recovery,
                step,
                limit: context.budget().max_graph_steps(),
            },
        })?;
        self.audit_invocation(
            context,
            &event,
            HarnessOperation::EnterGraphNode,
            AuditPhase::Lifecycle,
            OperationEffect::StateCommitted,
        )
        .await?;
        Ok(attempt_id)
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn complete_graph_node(
        &self,
        context: &mut RunContext,
        attempt_id: GraphNodeAttemptId,
        node_id: GraphNodeId,
        transition: GraphTransitionKey,
        next_node: GraphNodeId,
        next_kind: GraphNodeKind,
        next_recovery: GraphRecoveryMode,
    ) -> Result<(), HarnessError> {
        self.preflight(context, HarnessOperation::CompleteGraphNode)
            .await?;
        let event = context.next_event(AgentEventKind::Graph {
            event: GraphProgressEvent::GraphNodeCompleted {
                attempt_id,
                node_id,
                transition,
                next_node,
                next_kind,
                next_recovery,
            },
        })?;
        self.audit_invocation(
            context,
            &event,
            HarnessOperation::CompleteGraphNode,
            AuditPhase::Lifecycle,
            OperationEffect::StateCommitted,
        )
        .await
    }

    pub async fn complete_graph(
        &self,
        context: &mut RunContext,
        attempt_id: GraphNodeAttemptId,
        node_id: GraphNodeId,
    ) -> Result<(), HarnessError> {
        self.preflight(context, HarnessOperation::FinishGraph)
            .await?;
        context.commit_finished(RunOutcome::Completed)?;
        let event = context.next_event(AgentEventKind::Graph {
            event: GraphProgressEvent::GraphCompleted {
                attempt_id,
                node_id,
                steps: context.usage().graph_steps(),
            },
        })?;
        self.audit_lifecycle(context, &event, HarnessOperation::FinishGraph)
            .await
    }

    pub async fn fail_graph(
        &self,
        context: &mut RunContext,
        attempt_id: GraphNodeAttemptId,
        node_id: GraphNodeId,
        failure: GraphFailureKind,
    ) -> Result<(), HarnessError> {
        self.preflight(context, HarnessOperation::FinishGraph)
            .await?;
        let run_failure = RunFailureKind::Graph { kind: failure };
        context.commit_finished(RunOutcome::Failed { kind: run_failure })?;
        let event = context.next_event(AgentEventKind::Graph {
            event: GraphProgressEvent::GraphFailed {
                attempt_id,
                node_id,
                steps: context.usage().graph_steps(),
                failure,
                run_failure,
            },
        })?;
        self.audit_lifecycle(context, &event, HarnessOperation::FinishGraph)
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

        match self.capability_policy.authorize(capability) {
            AuthorizationDecision::Allowed => {}
            AuthorizationDecision::RequiresApproval => {
                self.record_policy_denied(
                    context,
                    call.id(),
                    tool_name,
                    capability,
                    HarnessOperation::InvokeTool,
                )
                .await?;
                return Err(HarnessError::PolicyDenied(PolicyDenial::new(
                    capability,
                    PolicyDenialReason::ApprovalRequiredButUnsupported,
                )));
            }
            AuthorizationDecision::Denied(denial) => {
                self.record_policy_denied(
                    context,
                    call.id(),
                    tool_name,
                    capability,
                    HarnessOperation::InvokeTool,
                )
                .await?;
                return Err(HarnessError::PolicyDenied(denial));
            }
        }

        match (capability, binding.execution().clone()) {
            (CapabilityKind::ReadOnly, ExecutionBinding::Direct(port)) => {
                self.invoke_direct_tool(context, port, call, tool_name, capability)
                    .await
            }
            (CapabilityKind::ReadOnly, ExecutionBinding::ManagedDirect(port)) => {
                self.invoke_managed_tool(context, port, call, tool_name, capability)
                    .await
            }
            (CapabilityKind::ReadOnly, ExecutionBinding::Contained(port)) => {
                self.invoke_contained_read_only(context, port, call, tool_name, capability)
                    .await
            }
            (CapabilityKind::LocalWrite, ExecutionBinding::Contained(_))
            | (CapabilityKind::LocalWrite, ExecutionBinding::Direct(_))
            | (CapabilityKind::LocalWrite, ExecutionBinding::ManagedDirect(_))
            | (CapabilityKind::ExternalWrite, _)
            | (CapabilityKind::Privileged, _) => {
                self.record_policy_denied(
                    context,
                    call.id(),
                    tool_name,
                    capability,
                    HarnessOperation::InvokeTool,
                )
                .await?;
                Err(HarnessError::CapabilityNotExecutable { capability })
            }
        }
    }

    async fn invoke_direct_tool(
        &self,
        context: &mut RunContext,
        port: Arc<dyn ToolPort>,
        call: ToolCall,
        tool_name: ToolName,
        capability: CapabilityKind,
    ) -> Result<ToolResult, HarnessError> {
        self.reserve_and_record_tool_started(
            context,
            call.id(),
            tool_name,
            capability,
            HarnessOperation::InvokeTool,
            ToolStartAudit::Configured,
        )
        .await?;
        let tool_call_id = call.id();
        let result = self.invoke_tool_port(context, &port, call).await;
        self.finish_tool_invocation(
            context,
            tool_call_id,
            HarnessOperation::InvokeTool,
            result,
            ToolTerminalAudit::Configured,
        )
        .await
    }

    async fn invoke_contained_read_only(
        &self,
        context: &mut RunContext,
        port: Arc<dyn ContainedToolPort>,
        call: ToolCall,
        tool_name: ToolName,
        capability: CapabilityKind,
    ) -> Result<ToolResult, HarnessError> {
        self.reserve_and_record_tool_started(
            context,
            call.id(),
            tool_name,
            capability,
            HarnessOperation::InvokeTool,
            ToolStartAudit::Configured,
        )
        .await?;
        let tool_call_id = call.id();
        let result = self.invoke_contained_tool_port(context, &port, call).await;
        self.finish_tool_invocation(
            context,
            tool_call_id,
            HarnessOperation::InvokeTool,
            result,
            ToolTerminalAudit::Configured,
        )
        .await
    }

    async fn invoke_managed_tool(
        &self,
        context: &mut RunContext,
        port: Arc<dyn crate::ManagedToolPort>,
        call: ToolCall,
        tool_name: ToolName,
        capability: CapabilityKind,
    ) -> Result<ToolResult, HarnessError> {
        self.reserve_and_record_tool_started(
            context,
            call.id(),
            tool_name,
            capability,
            HarnessOperation::InvokeTool,
            ToolStartAudit::Configured,
        )
        .await?;
        let tool_call_id = call.id();
        let result = self.invoke_managed_tool_port(context, &port, call).await;
        self.finish_tool_invocation(
            context,
            tool_call_id,
            HarnessOperation::InvokeTool,
            result,
            ToolTerminalAudit::Configured,
        )
        .await
    }

    async fn invoke_approved_local_write(
        &self,
        context: &mut RunContext,
        port: Arc<dyn ContainedToolPort>,
        call: ToolCall,
        meta: ApprovedLocalWriteMeta,
    ) -> Result<ToolResult, HarnessError> {
        let preview = match port.approval_preview(call.input()) {
            Ok(preview) => preview,
            Err(error) => {
                self.record_containment_failed(
                    context,
                    call.id(),
                    meta.tool_name.clone(),
                    meta.capability,
                    containment_failure_kind(error),
                )
                .await?;
                return Err(HarnessError::ContainmentPort(error));
            }
        };
        self.preflight(context, HarnessOperation::RequestApproval)
            .await?;
        let approval = Arc::clone(
            self.approval
                .as_ref()
                .ok_or(HarnessError::ApprovalPortMissing)?,
        );
        let approval_request_id = ApprovalRequestId::new();
        if let Err(error) = context.reserve_approval_request() {
            self.terminalize_safety(
                context,
                SafetyTerminalization::BudgetExceeded(error.dimension()),
            )
            .await;
            return Err(error.into());
        }
        let request = ApprovalRequest::new(
            approval_request_id,
            meta.action_proposal_id,
            call.id(),
            meta.tool_name.clone(),
            meta.capability,
            meta.action_digest,
            preview,
        );
        let event = context.next_event(AgentEventKind::ApprovalRequested {
            approval_request_id,
            action_proposal_id: meta.action_proposal_id,
            tool_call_id: call.id(),
            tool_name: meta.tool_name.clone(),
            capability: meta.capability,
            usage: context.usage().approval_requests(),
            limit: context.budget().max_approval_requests(),
        })?;
        self.audit_required(
            context,
            &event,
            HarnessOperation::RequestApproval,
            AuditPhase::BeforeInvocation,
            OperationEffect::NotInvoked,
        )
        .await?;

        let decision = match self.await_approval(context, &approval, request).await {
            Ok(decision) => decision,
            Err(HarnessError::ApprovalPort(error)) => {
                let kind = match error {
                    ApprovalPortError::Unavailable => ApprovalFailureKind::PortUnavailable,
                    ApprovalPortError::Failed => ApprovalFailureKind::PortFailed,
                };
                let event = context.next_event(AgentEventKind::ApprovalFailed {
                    approval_request_id,
                    kind,
                })?;
                self.audit_required(
                    context,
                    &event,
                    HarnessOperation::RequestApproval,
                    AuditPhase::AfterInvocation,
                    OperationEffect::NotInvoked,
                )
                .await?;
                return Err(HarnessError::ApprovalPort(error));
            }
            Err(error) => return Err(error),
        };
        self.preflight(context, HarnessOperation::RequestApproval)
            .await?;
        if decision.request_id() != approval_request_id
            || decision.action_digest() != meta.action_digest
        {
            let event = context.next_event(AgentEventKind::ApprovalFailed {
                approval_request_id,
                kind: ApprovalFailureKind::DecisionMismatch,
            })?;
            self.audit_required(
                context,
                &event,
                HarnessOperation::RequestApproval,
                AuditPhase::AfterInvocation,
                OperationEffect::NotInvoked,
            )
            .await?;
            return Err(HarnessError::ApprovalDecisionMismatch);
        }

        match decision.outcome() {
            ApprovalOutcome::Denied => {
                let event = context.next_event(AgentEventKind::ApprovalDenied {
                    approval_request_id,
                })?;
                self.audit_required(
                    context,
                    &event,
                    HarnessOperation::RequestApproval,
                    AuditPhase::AfterInvocation,
                    OperationEffect::NotInvoked,
                )
                .await?;
                Err(HarnessError::ApprovalDenied)
            }
            ApprovalOutcome::Granted => {
                let event = context.next_event(AgentEventKind::ApprovalGranted {
                    approval_request_id,
                })?;
                self.audit_required(
                    context,
                    &event,
                    HarnessOperation::RequestApproval,
                    AuditPhase::AfterInvocation,
                    OperationEffect::NotInvoked,
                )
                .await?;
                self.preflight(context, HarnessOperation::InvokeTool)
                    .await?;
                self.reserve_and_record_tool_started(
                    context,
                    call.id(),
                    meta.tool_name,
                    meta.capability,
                    HarnessOperation::InvokeTool,
                    ToolStartAudit::Required,
                )
                .await?;
                let tool_call_id = call.id();
                let result = self.invoke_contained_tool_port(context, &port, call).await;
                self.finish_tool_invocation(
                    context,
                    tool_call_id,
                    HarnessOperation::InvokeTool,
                    result,
                    ToolTerminalAudit::Required,
                )
                .await
            }
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

    async fn invoke_contained_tool_port(
        &self,
        context: &mut RunContext,
        port: &Arc<dyn ContainedToolPort>,
        call: ToolCall,
    ) -> Result<ToolResult, HarnessError> {
        let cancellation = context.cancellation()?;
        let deadline = context.deadline_at()?;
        let mut invocation = port
            .start_contained(call)
            .map_err(HarnessError::ContainmentPort)?;
        tokio::select! {
            biased;
            () = cancellation.cancelled() => {
                invocation
                    .terminate_and_reap()
                    .await
                    .map_err(HarnessError::ContainmentPort)?;
                self.terminalize_safety(context, SafetyTerminalization::Cancelled).await;
                Err(HarnessError::Cancelled { stage: ExecutionStage::Invocation })
            }
            () = sleep_until(deadline) => {
                invocation
                    .terminate_and_reap()
                    .await
                    .map_err(HarnessError::ContainmentPort)?;
                self.terminalize_safety(context, SafetyTerminalization::DeadlineExceeded).await;
                Err(HarnessError::DeadlineExceeded { stage: ExecutionStage::Invocation })
            }
            result = invocation.wait() => {
                result.map_err(HarnessError::ContainmentPort)
            },
        }
    }

    async fn invoke_managed_tool_port(
        &self,
        context: &mut RunContext,
        port: &Arc<dyn crate::ManagedToolPort>,
        call: ToolCall,
    ) -> Result<ToolResult, HarnessError> {
        let cancellation = context.cancellation()?;
        let deadline = context.deadline_at()?;
        let mut invocation = port.start_managed(call).map_err(HarnessError::ToolPort)?;
        tokio::select! {
            biased;
            () = cancellation.cancelled() => {
                invocation.terminate_and_reap().await.map_err(HarnessError::ToolPort)?;
                self.terminalize_safety(context, SafetyTerminalization::Cancelled).await;
                Err(HarnessError::Cancelled { stage: ExecutionStage::Invocation })
            }
            () = sleep_until(deadline) => {
                invocation.terminate_and_reap().await.map_err(HarnessError::ToolPort)?;
                self.terminalize_safety(context, SafetyTerminalization::DeadlineExceeded).await;
                Err(HarnessError::DeadlineExceeded { stage: ExecutionStage::Invocation })
            }
            result = invocation.wait() => result.map_err(HarnessError::ToolPort),
        }
    }

    async fn await_approval(
        &self,
        context: &mut RunContext,
        approval: &Arc<dyn ApprovalPort>,
        request: ApprovalRequest,
    ) -> Result<crate::ApprovalDecision, HarnessError> {
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
            result = approval.decide(&request) => {
                result.map_err(HarnessError::ApprovalPort)
            },
        }
    }

    async fn reserve_and_record_tool_started(
        &self,
        context: &mut RunContext,
        tool_call_id: ToolCallId,
        tool_name: ToolName,
        capability: CapabilityKind,
        operation: HarnessOperation,
        audit_mode: ToolStartAudit,
    ) -> Result<(), HarnessError> {
        if let Err(error) = context.reserve_tool_call() {
            self.terminalize_safety(
                context,
                SafetyTerminalization::BudgetExceeded(error.dimension()),
            )
            .await;
            return Err(error.into());
        }
        let event = context.next_event(AgentEventKind::ToolInvocationStarted {
            tool_call_id,
            tool_name,
            capability,
            usage: context.usage().tool_calls(),
            limit: context.budget().max_tool_calls(),
        })?;
        match audit_mode {
            ToolStartAudit::Configured => {
                self.audit_invocation(
                    context,
                    &event,
                    operation,
                    AuditPhase::BeforeInvocation,
                    OperationEffect::NotInvoked,
                )
                .await
            }
            ToolStartAudit::Required => {
                self.audit_required(
                    context,
                    &event,
                    operation,
                    AuditPhase::BeforeInvocation,
                    OperationEffect::NotInvoked,
                )
                .await
            }
        }
    }

    async fn finish_tool_invocation(
        &self,
        context: &mut RunContext,
        tool_call_id: ToolCallId,
        operation: HarnessOperation,
        result: Result<ToolResult, HarnessError>,
        audit_mode: ToolTerminalAudit,
    ) -> Result<ToolResult, HarnessError> {
        match result {
            Ok(result) => {
                if result.call_id() != tool_call_id {
                    self.record_tool_terminal_event(
                        context,
                        AgentEventKind::ToolInvocationAdapterFailed { tool_call_id },
                        operation,
                        audit_mode,
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
                self.record_tool_terminal_event(context, event_kind, operation, audit_mode)
                    .await?;
                Ok(result)
            }
            Err(HarnessError::ToolPort(error)) => {
                self.record_tool_terminal_event(
                    context,
                    AgentEventKind::ToolInvocationAdapterFailed { tool_call_id },
                    operation,
                    audit_mode,
                )
                .await?;
                Err(HarnessError::ToolPort(error))
            }
            Err(HarnessError::ContainmentPort(error)) => {
                self.record_tool_terminal_event(
                    context,
                    AgentEventKind::ToolInvocationAdapterFailed { tool_call_id },
                    operation,
                    audit_mode,
                )
                .await?;
                Err(HarnessError::ContainmentPort(error))
            }
            Err(error) => {
                self.record_tool_terminal_after_runtime_error(
                    context,
                    tool_call_id,
                    operation,
                    audit_mode,
                )
                .await?;
                Err(error)
            }
        }
    }

    async fn record_tool_terminal_after_runtime_error(
        &self,
        context: &mut RunContext,
        tool_call_id: ToolCallId,
        operation: HarnessOperation,
        audit_mode: ToolTerminalAudit,
    ) -> Result<(), HarnessError> {
        let event_kind = AgentEventKind::ToolInvocationAdapterFailed { tool_call_id };
        if matches!(context.status(), RunStatus::Running) {
            return self
                .record_tool_terminal_event(context, event_kind, operation, audit_mode)
                .await;
        }

        match context.next_event(event_kind) {
            Ok(event) => self.audit_terminal_best_effort(context, &event).await,
            Err(_) => context.mark_audit_degraded(),
        }
        Ok(())
    }

    async fn record_tool_terminal_event(
        &self,
        context: &mut RunContext,
        event_kind: AgentEventKind,
        operation: HarnessOperation,
        audit_mode: ToolTerminalAudit,
    ) -> Result<(), HarnessError> {
        let event = context.next_event(event_kind)?;
        match audit_mode {
            ToolTerminalAudit::Configured => {
                self.audit_invocation(
                    context,
                    &event,
                    operation,
                    AuditPhase::AfterInvocation,
                    OperationEffect::InvocationStarted,
                )
                .await
            }
            ToolTerminalAudit::Required => {
                self.audit_required(
                    context,
                    &event,
                    operation,
                    AuditPhase::AfterInvocation,
                    OperationEffect::InvocationStarted,
                )
                .await
            }
        }
    }

    async fn record_knowledge_failure(
        &self,
        context: &mut RunContext,
        retrieval_id: KnowledgeRetrievalId,
        kind: KnowledgeFailureKind,
    ) -> Result<(), HarnessError> {
        let event =
            context.next_event(AgentEventKind::KnowledgeRetrievalFailed { retrieval_id, kind })?;
        self.audit_invocation(
            context,
            &event,
            HarnessOperation::RetrieveKnowledge,
            AuditPhase::AfterInvocation,
            OperationEffect::InvocationStarted,
        )
        .await
    }

    async fn record_knowledge_interruption(
        &self,
        context: &mut RunContext,
        retrieval_id: KnowledgeRetrievalId,
        kind: KnowledgeFailureKind,
    ) -> Result<(), HarnessError> {
        let event =
            context.next_event(AgentEventKind::KnowledgeRetrievalFailed { retrieval_id, kind })?;
        self.persist_transition(context, &event).await?;
        if !matches!(
            timeout(self.config.audit_timeout(), self.audit.record(&event)).await,
            Ok(Ok(()))
        ) {
            context.mark_audit_degraded();
        }
        Ok(())
    }

    async fn record_policy_denied(
        &self,
        context: &mut RunContext,
        tool_call_id: ToolCallId,
        tool_name: ToolName,
        capability: CapabilityKind,
        operation: HarnessOperation,
    ) -> Result<(), HarnessError> {
        let event = context.next_event(AgentEventKind::ToolPolicyDenied {
            tool_call_id,
            tool_name,
            capability,
        })?;
        self.audit_invocation(
            context,
            &event,
            operation,
            AuditPhase::BeforeInvocation,
            OperationEffect::NotInvoked,
        )
        .await
    }

    async fn record_containment_failed(
        &self,
        context: &mut RunContext,
        tool_call_id: ToolCallId,
        tool_name: ToolName,
        capability: CapabilityKind,
        kind: ContainmentFailureKind,
    ) -> Result<(), HarnessError> {
        let event = context.next_event(AgentEventKind::ContainmentFailed {
            tool_call_id,
            tool_name,
            capability,
            kind,
        })?;
        self.audit_required(
            context,
            &event,
            HarnessOperation::InvokeValidatedAction,
            AuditPhase::BeforeInvocation,
            OperationEffect::NotInvoked,
        )
        .await
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
        if context.persistence_failed() {
            return Err(HarnessError::Persistence(PersistencePortError::Failed));
        }
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
        self.persist_transition(context, event).await?;
        match self.audit_with_run_bounds(context, event).await {
            AuditOutcome::Recorded => Ok(()),
            AuditOutcome::Failed(kind) => {
                self.apply_audit_failure(context, operation, phase, effect, kind)
                    .await
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

    async fn audit_required(
        &self,
        context: &mut RunContext,
        event: &AgentEvent,
        operation: HarnessOperation,
        phase: AuditPhase,
        effect: OperationEffect,
    ) -> Result<(), HarnessError> {
        self.persist_transition(context, event).await?;
        match self.audit_with_run_bounds(context, event).await {
            AuditOutcome::Recorded => Ok(()),
            AuditOutcome::Failed(kind) => {
                self.record_audit_degraded(context).await?;
                Err(HarnessError::Audit {
                    phase,
                    operation,
                    effect,
                    kind,
                })
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

    async fn audit_already_persisted(
        &self,
        context: &mut RunContext,
        event: &AgentEvent,
        operation: HarnessOperation,
        phase: AuditPhase,
        effect: OperationEffect,
        required: bool,
    ) -> Result<(), HarnessError> {
        match self.audit_with_run_bounds(context, event).await {
            AuditOutcome::Recorded => Ok(()),
            AuditOutcome::Failed(kind) if required => {
                self.record_audit_degraded(context).await?;
                Err(HarnessError::Audit {
                    phase,
                    operation,
                    effect,
                    kind,
                })
            }
            AuditOutcome::Failed(kind) => {
                self.apply_audit_failure(context, operation, phase, effect, kind)
                    .await
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

    fn transition_for_event(
        &self,
        context: &RunContext,
        event: &AgentEvent,
        checkpoint: bool,
    ) -> crate::AppendTransition {
        let expected_sequence = match event.sequence().get() {
            0 => None,
            value => Some(EventSequence::new(value - 1)),
        };
        crate::AppendTransition::new(
            RunKey::new(context.run_id(), context.session_id()),
            expected_sequence,
            event.clone(),
            checkpoint.then(|| context.durable_checkpoint()),
        )
    }

    fn restore_local_write(
        &self,
        record: &DurableApprovalRecord,
        state: &crate::DurableRunState,
    ) -> Result<
        (
            ValidatedAction,
            Arc<dyn ContainedToolPort>,
            crate::ApprovalPreview,
        ),
        HarnessError,
    > {
        let seal = self.action_seal.as_ref().ok_or(HarnessError::ActionSeal(
            crate::ActionSealError::KeyUnavailable,
        ))?;
        let workspace_binding_id = self
            .workspace_binding_id
            .ok_or(HarnessError::DurableApprovalMismatch)?;
        let binding = record.binding();
        if binding.workspace_binding_id() != workspace_binding_id
            || binding.capsule_version() != crate::LOCAL_WRITE_CAPSULE_VERSION
            || !matches!(
                state.graph().map(crate::DurableGraphState::position),
                Some(DurableGraphPosition::Waiting {
                    attempt_id,
                    node_id,
                    wait_id,
                    approval_request_id,
                    action_proposal_id,
                    tool_call_id,
                    action_digest,
                }) if *attempt_id == binding.attempt_id()
                    && node_id == binding.node_id()
                    && *wait_id == binding.wait_id()
                    && *approval_request_id == binding.approval_request_id()
                    && *action_proposal_id == binding.action_proposal_id()
                    && *tool_call_id == binding.tool_call_id()
                    && *action_digest == binding.action_digest()
            )
            || !matches!(
                state.recovery_contract(),
                RecoveryContract::Graph { program_version, definition_digest }
                    if program_version == binding.program_version()
                        && definition_digest == binding.graph_digest().as_bytes()
            )
        {
            return Err(HarnessError::DurableApprovalMismatch);
        }
        let tool_name = ToolName::new(DURABLE_LOCAL_WRITE_TOOL_NAME)
            .map_err(|_| HarnessError::DurableApprovalMismatch)?;
        let registered = self
            .tools
            .get(&tool_name)
            .ok_or_else(|| HarnessError::ToolNotFound {
                name: tool_name.clone(),
            })?;
        if registered.definition().capability() != CapabilityKind::LocalWrite
            || compute_tool_contract_digest(registered.definition())
                != binding.tool_contract_digest()
            || self.capability_policy.authorize(CapabilityKind::LocalWrite)
                != AuthorizationDecision::RequiresApproval
            || state.audit_degraded()
        {
            return Err(HarnessError::DurableApprovalMismatch);
        }
        let ExecutionBinding::Contained(port) = registered.execution().clone() else {
            return Err(HarnessError::ContainmentUnavailable { name: tool_name });
        };
        let capsule = seal.open_local_write(binding, record.sealed_action())?;
        let arguments = json!({
            "relative_path": capsule.relative_path(),
            "content": capsule.content(),
        });
        validate_argument_limits(&arguments).map_err(|_| HarnessError::DurableApprovalMismatch)?;
        if !registered.validator().is_valid(&arguments) {
            return Err(HarnessError::DurableApprovalMismatch);
        }
        let digest = compute_action_digest(&tool_name, CapabilityKind::LocalWrite, &arguments);
        if digest != binding.action_digest() {
            return Err(HarnessError::DurableApprovalMismatch);
        }
        let input = agent_core::ToolInput::new(arguments);
        let preview = port
            .approval_preview(&input)
            .map_err(HarnessError::ContainmentPort)?;
        Ok((
            ValidatedAction::from_durable_parts(
                binding.action_proposal_id(),
                tool_name,
                input,
                CapabilityKind::LocalWrite,
                digest,
            ),
            port,
            preview,
        ))
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
        self.persist_transition(context, event).await?;
        match timeout(self.config.audit_timeout(), self.audit.record(event)).await {
            Ok(Ok(())) => Ok(()),
            Ok(Err(kind)) => {
                self.apply_audit_failure(
                    context,
                    operation,
                    AuditPhase::Lifecycle,
                    OperationEffect::StateCommitted,
                    kind,
                )
                .await
            }
            Err(_) => {
                self.apply_audit_failure(
                    context,
                    operation,
                    AuditPhase::Lifecycle,
                    OperationEffect::StateCommitted,
                    AuditPortError::TimedOut,
                )
                .await
            }
        }
    }

    async fn apply_audit_failure(
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
                self.record_audit_degraded(context).await?;
                Ok(())
            }
        }
    }

    async fn record_audit_degraded(&self, context: &mut RunContext) -> Result<(), HarnessError> {
        let event = context.next_event(AgentEventKind::AuditDegraded)?;
        context.mark_audit_degraded();
        self.persist_transition(context, &event).await
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
        if self.persist_transition(context, event).await.is_err() {
            context.mark_persistence_failed();
            return;
        }
        if !matches!(
            timeout(self.config.audit_timeout(), self.audit.record(event)).await,
            Ok(Ok(()))
        ) {
            context.mark_audit_degraded();
        }
    }

    async fn persist_transition(
        &self,
        context: &mut RunContext,
        event: &AgentEvent,
    ) -> Result<(), HarnessError> {
        let Some(persistence) = &self.persistence else {
            return Ok(());
        };
        let expected_sequence = match event.sequence().get() {
            0 => None,
            value => Some(EventSequence::new(value - 1)),
        };
        let checkpoint = context
            .durable_state()
            .is_quiescent()
            .then(|| context.durable_checkpoint());
        let transition = crate::AppendTransition::new(
            RunKey::new(context.run_id(), context.session_id()),
            expected_sequence,
            event.clone(),
            checkpoint,
        );
        if let Err(error) = persistence.append_transition(&transition).await {
            context.mark_persistence_failed();
            return Err(HarnessError::Persistence(error));
        }
        Ok(())
    }
}

fn audit_stage(phase: AuditPhase) -> ExecutionStage {
    match phase {
        AuditPhase::BeforeInvocation => ExecutionStage::PreInvocationAudit,
        AuditPhase::AfterInvocation => ExecutionStage::PostInvocationAudit,
        AuditPhase::Lifecycle => ExecutionStage::LifecycleAudit,
    }
}

const fn containment_failure_kind(error: crate::ContainmentPortError) -> ContainmentFailureKind {
    match error {
        crate::ContainmentPortError::Unavailable => ContainmentFailureKind::Unavailable,
        crate::ContainmentPortError::PreviewRejected => ContainmentFailureKind::PreviewRejected,
        crate::ContainmentPortError::Infrastructure => ContainmentFailureKind::Infrastructure,
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

enum KnowledgeWaitOutcome {
    Result(Result<EvidenceSet, KnowledgeError>),
    Cancelled,
    DeadlineExceeded,
}

const fn knowledge_failure_kind(error: KnowledgeError) -> KnowledgeFailureKind {
    match error {
        KnowledgeError::Unavailable(_) => KnowledgeFailureKind::Unavailable,
        KnowledgeError::Rejected(_) => KnowledgeFailureKind::Rejected,
        KnowledgeError::MalformedResponse => KnowledgeFailureKind::MalformedResponse,
        KnowledgeError::SnapshotMismatch => KnowledgeFailureKind::SnapshotMismatch,
        KnowledgeError::Failed(_) | KnowledgeError::AllBackendsFailed => {
            KnowledgeFailureKind::Failed
        }
    }
}

#[derive(Clone, Copy)]
enum SafetyTerminalization {
    Cancelled,
    DeadlineExceeded,
    BudgetExceeded(BudgetDimension),
}

#[derive(Clone, Copy)]
enum ToolStartAudit {
    Configured,
    Required,
}

#[derive(Clone, Copy)]
enum ToolTerminalAudit {
    Configured,
    Required,
}

struct ApprovedLocalWriteMeta {
    action_proposal_id: ActionProposalId,
    tool_name: ToolName,
    capability: CapabilityKind,
    action_digest: ActionDigest,
}

const DURABLE_LOCAL_WRITE_TOOL_NAME: &str = "workspace_write_file";

fn local_write_capsule(value: &Value) -> Result<crate::LocalWriteActionCapsuleV1, HarnessError> {
    let object = value
        .as_object()
        .filter(|object| object.len() == 2)
        .ok_or(HarnessError::DurableApprovalMismatch)?;
    let relative_path = object
        .get("relative_path")
        .and_then(Value::as_str)
        .ok_or(HarnessError::DurableApprovalMismatch)?;
    let content = object
        .get("content")
        .and_then(Value::as_str)
        .ok_or(HarnessError::DurableApprovalMismatch)?;
    crate::LocalWriteActionCapsuleV1::new(relative_path.to_owned(), content.to_owned())
        .map_err(HarnessError::ActionSeal)
}

type WaitingBinding = (
    GraphNodeAttemptId,
    ApprovalRequestId,
    ActionProposalId,
    ToolCallId,
    ActionDigest,
);

fn waiting_position(
    state: &crate::DurableRunState,
    expected_wait_id: DurableApprovalWaitId,
) -> Result<WaitingBinding, HarnessError> {
    match state.graph().map(crate::DurableGraphState::position) {
        Some(DurableGraphPosition::Waiting {
            attempt_id,
            wait_id,
            approval_request_id,
            action_proposal_id,
            tool_call_id,
            action_digest,
            ..
        }) if *wait_id == expected_wait_id => Ok((
            *attempt_id,
            *approval_request_id,
            *action_proposal_id,
            *tool_call_id,
            *action_digest,
        )),
        _ => Err(HarnessError::DurableApprovalMismatch),
    }
}
