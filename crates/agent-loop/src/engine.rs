use agent_core::{
    LoopDecisionKind, LoopFailureKind, LoopPhase, LoopProgressEvent, RunFailureKind, RunStatus,
};
use agent_harness::{ExecutionHarness, HarnessError, RunContext};

use crate::{
    LoopEffects, LoopError, LoopProgram, LoopState, LoopStepError, LoopTransitionError,
    ReflectDecision, RestartableLoopProgram, VerificationResult,
};

#[derive(Clone, Copy, Debug, Default)]
pub struct LoopEngine;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TerminalLoopDecision {
    Complete,
    Fail { kind: LoopFailureKind },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoopRunSummary {
    reserved_iterations: u32,
    completed_iterations: u32,
    terminal_decision: TerminalLoopDecision,
    final_status: RunStatus,
}

impl LoopRunSummary {
    #[must_use]
    pub const fn reserved_iterations(&self) -> u32 {
        self.reserved_iterations
    }

    #[must_use]
    pub const fn completed_iterations(&self) -> u32 {
        self.completed_iterations
    }

    #[must_use]
    pub const fn terminal_decision(&self) -> TerminalLoopDecision {
        self.terminal_decision
    }

    #[must_use]
    pub const fn final_status(&self) -> &RunStatus {
        &self.final_status
    }
}

impl LoopEngine {
    #[must_use]
    pub const fn new() -> Self {
        Self
    }

    /// Drives an already-running context until a typed Reflect decision ends
    /// the loop or the harness makes the run terminal.
    ///
    /// The caller owns `start_run` so it can retain the cancellation handle.
    /// This engine owns normal `complete_run` / `fail_run` finalization.
    pub async fn run<P: LoopProgram>(
        &self,
        harness: &ExecutionHarness,
        context: &mut RunContext,
        program: &mut P,
        working_state: &mut P::WorkingState,
    ) -> Result<LoopRunSummary, LoopError> {
        self.run_with_state(harness, context, program, working_state, LoopState::new())
            .await
    }

    /// Continues only a run classified as resumable from metadata-only state.
    pub async fn resume<P: RestartableLoopProgram>(
        &self,
        harness: &ExecutionHarness,
        recovered: agent_harness::RecoveredRun,
        program: &mut P,
    ) -> Result<(LoopRunSummary, P::WorkingState), LoopError> {
        let (mut context, durable, contract) = recovered.into_parts();
        let contract_matches = match contract {
            agent_harness::RecoveryContract::Restartable { version } => {
                version == P::RECOVERY_VERSION
            }
            agent_harness::RecoveryContract::RestartableRetrieval { version } => {
                version == P::RECOVERY_VERSION && P::RESTART_INTERRUPTED_RETRIEVAL
            }
            agent_harness::RecoveryContract::NonRestartable => false,
        };
        if !contract_matches {
            return Err(LoopError::RecoveryContractMismatch);
        }
        let loop_state = match durable.loop_position() {
            agent_harness::DurableLoopPosition::NotStarted => LoopState::new(),
            agent_harness::DurableLoopPosition::Ready => {
                LoopState::recovered_ready(durable.completed_iterations())
            }
            agent_harness::DurableLoopPosition::PhaseRunning(LoopPhase::Retrieve)
                if P::RESTART_INTERRUPTED_RETRIEVAL
                    && durable.restartable_knowledge_retrieval().is_some() =>
            {
                LoopState::recovered_retrieve_running(
                    durable
                        .current_iteration()
                        .ok_or(LoopError::RecoveryStateInvalid)?,
                    durable.completed_iterations(),
                )
            }
            _ => return Err(LoopError::RecoveryStateInvalid),
        };
        let mut working_state = program
            .restore_working_state(&durable)
            .map_err(|_| LoopError::RecoveryStateInvalid)?;
        let summary = self
            .run_with_state(
                harness,
                &mut context,
                program,
                &mut working_state,
                loop_state,
            )
            .await?;
        Ok((summary, working_state))
    }

    pub(crate) async fn run_with_state<P: LoopProgram>(
        &self,
        harness: &ExecutionHarness,
        context: &mut RunContext,
        program: &mut P,
        working_state: &mut P::WorkingState,
        mut state: LoopState,
    ) -> Result<LoopRunSummary, LoopError> {
        self.checkpoint(harness, context).await?;

        loop {
            let iteration = if matches!(
                state.position(),
                crate::LoopPosition::PhaseRunning(LoopPhase::Retrieve)
            ) {
                let iteration = state
                    .current_iteration()
                    .ok_or(LoopError::RecoveryStateInvalid)?;
                self.continue_retrieve(
                    harness,
                    context,
                    program,
                    working_state,
                    &mut state,
                    iteration,
                )
                .await?;
                iteration
            } else {
                let iteration = match harness.begin_iteration(context).await {
                    Ok(iteration) => iteration,
                    Err(error) => {
                        return Err(self
                            .terminalize_harness_error(harness, context, error)
                            .await);
                    }
                };
                self.apply_transition(
                    harness,
                    context,
                    iteration,
                    state.begin_iteration(iteration),
                )
                .await?;

                self.run_observe(
                    harness,
                    context,
                    program,
                    working_state,
                    &mut state,
                    iteration,
                )
                .await?;
                self.run_retrieve(
                    harness,
                    context,
                    program,
                    working_state,
                    &mut state,
                    iteration,
                )
                .await?;
                iteration
            };
            self.run_plan(
                harness,
                context,
                program,
                working_state,
                &mut state,
                iteration,
            )
            .await?;
            self.run_act(
                harness,
                context,
                program,
                working_state,
                &mut state,
                iteration,
            )
            .await?;
            let verification = self
                .run_verify(
                    harness,
                    context,
                    program,
                    working_state,
                    &mut state,
                    iteration,
                )
                .await?;
            let decision = self
                .run_reflect(
                    harness,
                    context,
                    program,
                    working_state,
                    &mut state,
                    iteration,
                    verification,
                )
                .await?;

            self.record_progress(
                harness,
                context,
                LoopProgressEvent::ReflectDecision {
                    iteration,
                    decision: decision.kind(),
                },
            )
            .await?;
            self.record_progress(
                harness,
                context,
                LoopProgressEvent::IterationCompleted { iteration },
            )
            .await?;

            match decision {
                ReflectDecision::Continue => continue,
                ReflectDecision::Complete => {
                    self.record_progress(
                        harness,
                        context,
                        LoopProgressEvent::LoopCompleted {
                            completed_iterations: state.completed_iterations(),
                        },
                    )
                    .await?;
                    self.complete_run(harness, context).await?;
                    return Ok(summary(context, &state, TerminalLoopDecision::Complete));
                }
                ReflectDecision::Fail { kind } => {
                    let run_failure = RunFailureKind::Loop { kind };
                    self.record_progress(
                        harness,
                        context,
                        LoopProgressEvent::LoopFailed {
                            iteration,
                            kind: run_failure,
                        },
                    )
                    .await?;
                    self.fail_run(harness, context, run_failure).await?;
                    return Ok(summary(
                        context,
                        &state,
                        TerminalLoopDecision::Fail { kind },
                    ));
                }
            }
        }
    }

    async fn run_observe<P: LoopProgram>(
        &self,
        harness: &ExecutionHarness,
        context: &mut RunContext,
        program: &mut P,
        working_state: &mut P::WorkingState,
        state: &mut LoopState,
        iteration: u32,
    ) -> Result<(), LoopError> {
        self.enter_phase(harness, context, state, iteration, LoopPhase::Observe)
            .await?;
        let result = program
            .observe(iteration, working_state, LoopEffects::new(harness, context))
            .await;
        self.finish_unit_phase(
            harness,
            context,
            state,
            iteration,
            LoopPhase::Observe,
            result,
        )
        .await
    }

    async fn run_retrieve<P: LoopProgram>(
        &self,
        harness: &ExecutionHarness,
        context: &mut RunContext,
        program: &mut P,
        working_state: &mut P::WorkingState,
        state: &mut LoopState,
        iteration: u32,
    ) -> Result<(), LoopError> {
        self.enter_phase(harness, context, state, iteration, LoopPhase::Retrieve)
            .await?;
        self.continue_retrieve(harness, context, program, working_state, state, iteration)
            .await
    }

    async fn continue_retrieve<P: LoopProgram>(
        &self,
        harness: &ExecutionHarness,
        context: &mut RunContext,
        program: &mut P,
        working_state: &mut P::WorkingState,
        state: &mut LoopState,
        iteration: u32,
    ) -> Result<(), LoopError> {
        let result = program
            .retrieve(iteration, working_state, LoopEffects::new(harness, context))
            .await;
        self.finish_unit_phase(
            harness,
            context,
            state,
            iteration,
            LoopPhase::Retrieve,
            result,
        )
        .await
    }

    async fn run_plan<P: LoopProgram>(
        &self,
        harness: &ExecutionHarness,
        context: &mut RunContext,
        program: &mut P,
        working_state: &mut P::WorkingState,
        state: &mut LoopState,
        iteration: u32,
    ) -> Result<(), LoopError> {
        self.enter_phase(harness, context, state, iteration, LoopPhase::Plan)
            .await?;
        let result = program
            .plan(iteration, working_state, LoopEffects::new(harness, context))
            .await;
        self.finish_unit_phase(harness, context, state, iteration, LoopPhase::Plan, result)
            .await
    }

    async fn run_act<P: LoopProgram>(
        &self,
        harness: &ExecutionHarness,
        context: &mut RunContext,
        program: &mut P,
        working_state: &mut P::WorkingState,
        state: &mut LoopState,
        iteration: u32,
    ) -> Result<(), LoopError> {
        self.enter_phase(harness, context, state, iteration, LoopPhase::Act)
            .await?;
        let result = program
            .act(iteration, working_state, LoopEffects::new(harness, context))
            .await;
        self.finish_unit_phase(harness, context, state, iteration, LoopPhase::Act, result)
            .await
    }

    async fn run_verify<P: LoopProgram>(
        &self,
        harness: &ExecutionHarness,
        context: &mut RunContext,
        program: &mut P,
        working_state: &mut P::WorkingState,
        state: &mut LoopState,
        iteration: u32,
    ) -> Result<VerificationResult, LoopError> {
        self.enter_phase(harness, context, state, iteration, LoopPhase::Verify)
            .await?;
        let result = program
            .verify(iteration, working_state, LoopEffects::new(harness, context))
            .await;
        self.require_running(context)?;
        let verification = match result {
            Ok(verification) => verification,
            Err(error) => {
                return Err(self
                    .terminalize_step_error(harness, context, LoopPhase::Verify, error)
                    .await);
            }
        };
        self.checkpoint(harness, context).await?;
        self.apply_transition(
            harness,
            context,
            iteration,
            state.complete_phase(LoopPhase::Verify),
        )
        .await?;
        self.record_progress(
            harness,
            context,
            LoopProgressEvent::PhaseCompleted {
                iteration,
                phase: LoopPhase::Verify,
            },
        )
        .await?;
        Ok(verification)
    }

    #[allow(clippy::too_many_arguments)]
    async fn run_reflect<P: LoopProgram>(
        &self,
        harness: &ExecutionHarness,
        context: &mut RunContext,
        program: &mut P,
        working_state: &mut P::WorkingState,
        state: &mut LoopState,
        iteration: u32,
        verification: VerificationResult,
    ) -> Result<ReflectDecision, LoopError> {
        self.enter_phase(harness, context, state, iteration, LoopPhase::Reflect)
            .await?;
        let result = program
            .reflect(
                iteration,
                working_state,
                verification,
                LoopEffects::new(harness, context),
            )
            .await;
        self.require_running(context)?;
        let decision = match result {
            Ok(decision) => decision,
            Err(error) => {
                return Err(self
                    .terminalize_step_error(harness, context, LoopPhase::Reflect, error)
                    .await);
            }
        };
        self.checkpoint(harness, context).await?;
        self.apply_transition(harness, context, iteration, state.accept_reflect(decision))
            .await?;
        self.record_progress(
            harness,
            context,
            LoopProgressEvent::PhaseCompleted {
                iteration,
                phase: LoopPhase::Reflect,
            },
        )
        .await?;
        Ok(decision)
    }

    async fn enter_phase(
        &self,
        harness: &ExecutionHarness,
        context: &mut RunContext,
        state: &mut LoopState,
        iteration: u32,
        phase: LoopPhase,
    ) -> Result<(), LoopError> {
        self.checkpoint(harness, context).await?;
        self.apply_transition(harness, context, iteration, state.enter_phase(phase))
            .await?;
        self.record_progress(
            harness,
            context,
            LoopProgressEvent::PhaseEntered { iteration, phase },
        )
        .await
    }

    async fn finish_unit_phase(
        &self,
        harness: &ExecutionHarness,
        context: &mut RunContext,
        state: &mut LoopState,
        iteration: u32,
        phase: LoopPhase,
        result: Result<(), LoopStepError>,
    ) -> Result<(), LoopError> {
        self.require_running(context)?;
        if let Err(error) = result {
            return Err(self
                .terminalize_step_error(harness, context, phase, error)
                .await);
        }
        self.checkpoint(harness, context).await?;
        self.apply_transition(harness, context, iteration, state.complete_phase(phase))
            .await?;
        self.record_progress(
            harness,
            context,
            LoopProgressEvent::PhaseCompleted { iteration, phase },
        )
        .await
    }

    async fn checkpoint(
        &self,
        harness: &ExecutionHarness,
        context: &mut RunContext,
    ) -> Result<(), LoopError> {
        match harness.checkpoint(context).await {
            Ok(()) => self.require_running(context),
            Err(error) => Err(self
                .terminalize_harness_error(harness, context, error)
                .await),
        }
    }

    async fn record_progress(
        &self,
        harness: &ExecutionHarness,
        context: &mut RunContext,
        progress: LoopProgressEvent,
    ) -> Result<(), LoopError> {
        match harness.record_loop_progress(context, progress).await {
            Ok(()) => self.require_running(context),
            Err(error) => Err(self
                .terminalize_harness_error(harness, context, error)
                .await),
        }
    }

    async fn apply_transition(
        &self,
        harness: &ExecutionHarness,
        context: &mut RunContext,
        iteration: u32,
        result: Result<(), LoopTransitionError>,
    ) -> Result<(), LoopError> {
        let Err(error) = result else {
            return Ok(());
        };

        if matches!(context.status(), RunStatus::Running) {
            let failure = RunFailureKind::Loop {
                kind: LoopFailureKind::InvariantViolation,
            };
            let _ = harness
                .record_loop_progress(
                    context,
                    LoopProgressEvent::LoopFailed {
                        iteration,
                        kind: failure,
                    },
                )
                .await;
            if matches!(context.status(), RunStatus::Running) {
                let _ = harness.fail_run(context, failure).await;
            }
        }
        Err(LoopError::Transition(error))
    }

    async fn terminalize_step_error(
        &self,
        harness: &ExecutionHarness,
        context: &mut RunContext,
        phase: LoopPhase,
        error: LoopStepError,
    ) -> LoopError {
        if !matches!(context.status(), RunStatus::Running) {
            return terminal_error(context);
        }

        match error {
            LoopStepError::Harness(error) => {
                self.terminalize_harness_error(harness, context, error)
                    .await
            }
            LoopStepError::Program => {
                let failure = RunFailureKind::Loop {
                    kind: LoopFailureKind::Program { phase },
                };
                if let Err(error) = self
                    .record_progress(
                        harness,
                        context,
                        LoopProgressEvent::LoopFailed {
                            iteration: context.usage().iterations(),
                            kind: failure,
                        },
                    )
                    .await
                {
                    return error;
                }
                match harness.fail_run(context, failure).await {
                    Ok(()) => LoopError::Program { phase },
                    Err(source) if matches!(context.status(), RunStatus::Finished(_)) => {
                        LoopError::Finalization { source }
                    }
                    Err(source) => LoopError::Harness(source),
                }
            }
        }
    }

    async fn terminalize_harness_error(
        &self,
        harness: &ExecutionHarness,
        context: &mut RunContext,
        mut error: HarnessError,
    ) -> LoopError {
        if !matches!(context.status(), RunStatus::Running) {
            return LoopError::Harness(error);
        }

        let mut failure = failure_kind_for_harness_error(&error);
        let iteration = context.usage().iterations();
        if let Err(progress_error) = harness
            .record_loop_progress(
                context,
                LoopProgressEvent::LoopFailed {
                    iteration,
                    kind: failure,
                },
            )
            .await
        {
            if !matches!(context.status(), RunStatus::Running) {
                return LoopError::Harness(progress_error);
            }
            failure = failure_kind_for_harness_error(&progress_error);
            error = progress_error;
        }

        match harness.fail_run(context, failure).await {
            Ok(()) => LoopError::Harness(error),
            Err(source) if matches!(context.status(), RunStatus::Finished(_)) => {
                LoopError::Finalization { source }
            }
            Err(source) => LoopError::Harness(source),
        }
    }

    async fn complete_run(
        &self,
        harness: &ExecutionHarness,
        context: &mut RunContext,
    ) -> Result<(), LoopError> {
        match harness.complete_run(context).await {
            Ok(()) => Ok(()),
            Err(source) if matches!(context.status(), RunStatus::Finished(_)) => {
                Err(LoopError::Finalization { source })
            }
            Err(error) => Err(self
                .terminalize_harness_error(harness, context, error)
                .await),
        }
    }

    async fn fail_run(
        &self,
        harness: &ExecutionHarness,
        context: &mut RunContext,
        failure: RunFailureKind,
    ) -> Result<(), LoopError> {
        match harness.fail_run(context, failure).await {
            Ok(()) => Ok(()),
            Err(source) if matches!(context.status(), RunStatus::Finished(_)) => {
                Err(LoopError::Finalization { source })
            }
            Err(error) => Err(self
                .terminalize_harness_error(harness, context, error)
                .await),
        }
    }

    fn require_running(&self, context: &RunContext) -> Result<(), LoopError> {
        if matches!(context.status(), RunStatus::Running) {
            Ok(())
        } else {
            Err(terminal_error(context))
        }
    }
}

impl ReflectDecision {
    const fn kind(self) -> LoopDecisionKind {
        match self {
            Self::Complete => LoopDecisionKind::Complete,
            Self::Continue => LoopDecisionKind::Continue,
            Self::Fail { .. } => LoopDecisionKind::Fail,
        }
    }
}

fn failure_kind_for_harness_error(error: &HarnessError) -> RunFailureKind {
    match error {
        HarnessError::Audit { .. } => RunFailureKind::AuditUnavailable,
        HarnessError::ModelPort(_) => RunFailureKind::Model,
        HarnessError::ToolPort(_)
        | HarnessError::ToolNotFound { .. }
        | HarnessError::PolicyDenied(_) => RunFailureKind::Tool,
        HarnessError::ApprovalRequired
        | HarnessError::ApprovalPortMissing
        | HarnessError::ApprovalPort(_)
        | HarnessError::ApprovalDecisionMismatch
        | HarnessError::ApprovalDenied => RunFailureKind::Approval,
        HarnessError::AuditDegraded
        | HarnessError::CapabilityNotExecutable { .. }
        | HarnessError::ContainmentUnavailable { .. }
        | HarnessError::ContainmentPort(_) => RunFailureKind::Containment,
        HarnessError::InvalidLifecycle { .. }
        | HarnessError::BudgetExceeded(_)
        | HarnessError::Cancelled { .. }
        | HarnessError::DeadlineExceeded { .. }
        | HarnessError::Persistence(_)
        | HarnessError::Recovery(_)
        | HarnessError::Context(_)
        | HarnessError::KnowledgePort(_)
        | HarnessError::GroundingMismatch => RunFailureKind::Internal,
    }
}

fn terminal_error(context: &RunContext) -> LoopError {
    LoopError::RunTerminal {
        status: context.status().clone(),
    }
}

fn summary(
    context: &RunContext,
    state: &LoopState,
    terminal_decision: TerminalLoopDecision,
) -> LoopRunSummary {
    LoopRunSummary {
        reserved_iterations: context.usage().iterations(),
        completed_iterations: state.completed_iterations(),
        terminal_decision,
        final_status: context.status().clone(),
    }
}
