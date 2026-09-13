use agent_core::{
    AgentEvent, AgentEventKind, BudgetDimension, BudgetUsage, EventSequence, RunBudget, RunId,
    RunOutcome, RunStateError, RunStatus, SessionId,
};
use thiserror::Error;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

pub struct RunContext {
    run_id: RunId,
    session_id: SessionId,
    budget: RunBudget,
    usage: BudgetUsage,
    status: RunStatus,
    next_event_sequence: EventSequence,
    runtime: Option<RunRuntime>,
    audit_degraded: bool,
    recovery_contract: crate::RecoveryContract,
    durable_state: crate::DurableRunState,
    persistence_failed: bool,
}

struct RunRuntime {
    _started_at: Instant,
    deadline_at: Instant,
    cancellation: CancellationToken,
}

impl RunContext {
    #[must_use]
    pub fn new(run_id: RunId, session_id: SessionId, budget: RunBudget) -> Self {
        Self::new_with_recovery_contract(
            run_id,
            session_id,
            budget,
            crate::RecoveryContract::NonRestartable,
        )
    }

    #[must_use]
    pub fn new_restartable(
        run_id: RunId,
        session_id: SessionId,
        budget: RunBudget,
        recovery_version: u32,
    ) -> Self {
        Self::new_with_recovery_contract(
            run_id,
            session_id,
            budget,
            crate::RecoveryContract::Restartable {
                version: recovery_version,
            },
        )
    }

    /// Creates a run whose program explicitly supports reconstructing and
    /// freshly reissuing an interrupted read-only knowledge retrieval.
    #[must_use]
    pub fn new_restartable_retrieval(
        run_id: RunId,
        session_id: SessionId,
        budget: RunBudget,
        recovery_version: u32,
    ) -> Self {
        Self::new_with_recovery_contract(
            run_id,
            session_id,
            budget,
            crate::RecoveryContract::RestartableRetrieval {
                version: recovery_version,
            },
        )
    }

    #[must_use]
    pub fn new_graph(
        run_id: RunId,
        session_id: SessionId,
        budget: RunBudget,
        program_version: u32,
        definition_digest: [u8; 32],
    ) -> Self {
        Self::new_with_recovery_contract(
            run_id,
            session_id,
            budget,
            crate::RecoveryContract::Graph {
                program_version,
                definition_digest,
            },
        )
    }

    fn new_with_recovery_contract(
        run_id: RunId,
        session_id: SessionId,
        budget: RunBudget,
        recovery_contract: crate::RecoveryContract,
    ) -> Self {
        let record = crate::RunRecord::new(
            crate::RunKey::new(run_id, session_id),
            budget,
            recovery_contract,
        );
        Self {
            run_id,
            session_id,
            budget,
            usage: BudgetUsage::with_approval_requests(0, 0, 0, 0),
            status: RunStatus::Pending,
            next_event_sequence: EventSequence::new(0),
            runtime: None,
            audit_degraded: false,
            recovery_contract,
            durable_state: crate::DurableRunState::initial(&record),
            persistence_failed: false,
        }
    }

    #[must_use]
    pub const fn run_id(&self) -> RunId {
        self.run_id
    }

    #[must_use]
    pub const fn session_id(&self) -> SessionId {
        self.session_id
    }

    #[must_use]
    pub const fn budget(&self) -> &RunBudget {
        &self.budget
    }

    #[must_use]
    pub const fn usage(&self) -> BudgetUsage {
        self.usage
    }

    #[must_use]
    pub const fn status(&self) -> &RunStatus {
        &self.status
    }

    #[must_use]
    pub const fn audit_degraded(&self) -> bool {
        self.audit_degraded
    }

    pub(crate) fn commit_running(
        &mut self,
        started_at: Instant,
        wall_clock: SystemTime,
    ) -> Result<RunCancellationHandle, RunContextError> {
        let status = self.status.clone().start()?;
        let deadline_at = started_at
            .checked_add(self.budget.max_elapsed())
            .ok_or(RunContextError::DeadlineOutOfRange)?;
        let cancellation = CancellationToken::new();
        let started_at_unix_millis = wall_clock
            .duration_since(UNIX_EPOCH)
            .map_err(|_| RunContextError::WallClockOutOfRange)?
            .as_millis()
            .try_into()
            .map_err(|_| RunContextError::WallClockOutOfRange)?;
        let handle = RunCancellationHandle {
            token: cancellation.clone(),
        };

        self.status = status;
        self.runtime = Some(RunRuntime {
            _started_at: started_at,
            deadline_at,
            cancellation,
        });
        self.durable_state.set_started_at(started_at_unix_millis);
        Ok(handle)
    }

    pub(crate) fn commit_finished(&mut self, outcome: RunOutcome) -> Result<(), RunContextError> {
        self.status = self.status.clone().finish(outcome)?;
        Ok(())
    }

    pub(crate) fn started_at_unix_millis(&self) -> Result<u64, RunContextError> {
        self.durable_state
            .started_at_unix_millis()
            .ok_or(RunContextError::WallClockOutOfRange)
    }

    pub(crate) fn reserve_model_call(&mut self) -> Result<(), BudgetExceeded> {
        let usage = self.usage.model_calls();
        let limit = self.budget.max_model_calls();
        if usage >= limit {
            return Err(BudgetExceeded::new(BudgetDimension::ModelCalls));
        }

        self.usage = BudgetUsage::with_approval_requests(
            usage + 1,
            self.usage.tool_calls(),
            self.usage.iterations(),
            self.usage.approval_requests(),
        )
        .with_graph_steps(self.usage.graph_steps());
        Ok(())
    }

    pub(crate) fn reserve_tool_call(&mut self) -> Result<(), BudgetExceeded> {
        let usage = self.usage.tool_calls();
        let limit = self.budget.max_tool_calls();
        if usage >= limit {
            return Err(BudgetExceeded::new(BudgetDimension::ToolCalls));
        }

        self.usage = BudgetUsage::with_approval_requests(
            self.usage.model_calls(),
            usage + 1,
            self.usage.iterations(),
            self.usage.approval_requests(),
        )
        .with_graph_steps(self.usage.graph_steps());
        Ok(())
    }

    pub(crate) fn reserve_iteration(&mut self) -> Result<u32, BudgetExceeded> {
        let usage = self.usage.iterations();
        let limit = self.budget.max_iterations();
        if usage >= limit {
            return Err(BudgetExceeded::new(BudgetDimension::Iterations));
        }

        let iteration = usage + 1;
        self.usage = BudgetUsage::with_approval_requests(
            self.usage.model_calls(),
            self.usage.tool_calls(),
            iteration,
            self.usage.approval_requests(),
        )
        .with_graph_steps(self.usage.graph_steps());
        Ok(iteration)
    }

    pub(crate) fn reserve_approval_request(&mut self) -> Result<(), BudgetExceeded> {
        let usage = self.usage.approval_requests();
        let limit = self.budget.max_approval_requests();
        if usage >= limit {
            return Err(BudgetExceeded::new(BudgetDimension::ApprovalRequests));
        }

        self.usage = BudgetUsage::with_approval_requests(
            self.usage.model_calls(),
            self.usage.tool_calls(),
            self.usage.iterations(),
            usage + 1,
        )
        .with_graph_steps(self.usage.graph_steps());
        Ok(())
    }

    pub(crate) fn reserve_graph_step(&mut self) -> Result<u32, BudgetExceeded> {
        let usage = self.usage.graph_steps();
        let limit = self.budget.max_graph_steps();
        if usage >= limit || limit == 0 || limit > agent_core::MAX_GRAPH_STEPS {
            return Err(BudgetExceeded::new(BudgetDimension::GraphSteps));
        }

        let step = usage + 1;
        self.usage = BudgetUsage::with_approval_requests(
            self.usage.model_calls(),
            self.usage.tool_calls(),
            self.usage.iterations(),
            self.usage.approval_requests(),
        )
        .with_graph_steps(step);
        Ok(step)
    }

    pub(crate) fn cancellation(&self) -> Result<CancellationToken, RunContextError> {
        Ok(self.runtime()?.cancellation.clone())
    }

    pub(crate) fn cancellation_handle(&self) -> Result<RunCancellationHandle, RunContextError> {
        Ok(RunCancellationHandle {
            token: self.cancellation()?,
        })
    }

    pub(crate) fn deadline_at(&self) -> Result<Instant, RunContextError> {
        Ok(self.runtime()?.deadline_at)
    }

    pub(crate) fn request_cancel(&self) -> Result<(), RunContextError> {
        self.runtime()?.cancellation.cancel();
        Ok(())
    }

    pub(crate) fn mark_audit_degraded(&mut self) {
        self.audit_degraded = true;
        self.durable_state.set_audit_degraded();
    }

    pub(crate) fn next_event(
        &mut self,
        kind: AgentEventKind,
    ) -> Result<AgentEvent, RunContextError> {
        let sequence = self.next_event_sequence;
        self.next_event_sequence = sequence
            .checked_next()
            .ok_or(RunContextError::EventSequenceExhausted)?;
        let event = AgentEvent::new(self.run_id, sequence, kind);
        self.durable_state
            .apply(&event)
            .map_err(|_| RunContextError::DurableTransitionInvalid)?;
        Ok(event)
    }

    pub(crate) fn run_record(&self) -> crate::RunRecord {
        crate::RunRecord::new(
            crate::RunKey::new(self.run_id, self.session_id),
            self.budget,
            self.recovery_contract,
        )
    }

    pub(crate) fn durable_checkpoint(&self) -> crate::DurableCheckpoint {
        crate::DurableCheckpoint::new(self.durable_state.clone())
    }

    pub(crate) const fn durable_state(&self) -> &crate::DurableRunState {
        &self.durable_state
    }

    pub const fn recovery_contract(&self) -> crate::RecoveryContract {
        self.recovery_contract
    }

    pub(crate) const fn persistence_failed(&self) -> bool {
        self.persistence_failed
    }

    pub(crate) fn mark_persistence_failed(&mut self) {
        self.persistence_failed = true;
    }

    pub(crate) fn from_recovery(
        state: &crate::DurableRunState,
        elapsed: Duration,
        now: Instant,
        recovery_contract: crate::RecoveryContract,
    ) -> Result<Self, RunContextError> {
        let remaining = state
            .budget()
            .max_elapsed()
            .checked_sub(elapsed)
            .ok_or(RunContextError::DeadlineOutOfRange)?;
        let deadline_at = now
            .checked_add(remaining)
            .ok_or(RunContextError::DeadlineOutOfRange)?;
        let next_event_sequence = match state.last_sequence() {
            Some(sequence) => sequence
                .checked_next()
                .ok_or(RunContextError::EventSequenceExhausted)?,
            None => EventSequence::new(0),
        };
        let cancellation = CancellationToken::new();
        Ok(Self {
            run_id: state.key().run_id(),
            session_id: state.key().session_id(),
            budget: *state.budget(),
            usage: state.usage(),
            status: state.status().clone(),
            next_event_sequence,
            runtime: matches!(state.status(), RunStatus::Running).then_some(RunRuntime {
                _started_at: now,
                deadline_at,
                cancellation,
            }),
            audit_degraded: state.audit_degraded(),
            recovery_contract,
            durable_state: state.clone(),
            persistence_failed: false,
        })
    }

    fn runtime(&self) -> Result<&RunRuntime, RunContextError> {
        self.runtime
            .as_ref()
            .ok_or(RunContextError::RuntimeUnavailable)
    }
}

#[derive(Clone)]
pub struct RunCancellationHandle {
    token: CancellationToken,
}

impl RunCancellationHandle {
    pub fn request_cancel(&self) {
        self.token.cancel();
    }

    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.token.is_cancelled()
    }
}

#[derive(Clone, Copy, Debug, Error, PartialEq, Eq)]
#[error("run budget exhausted for {dimension}")]
pub struct BudgetExceeded {
    dimension: BudgetDimension,
}

impl BudgetExceeded {
    #[must_use]
    pub const fn new(dimension: BudgetDimension) -> Self {
        Self { dimension }
    }

    #[must_use]
    pub const fn dimension(self) -> BudgetDimension {
        self.dimension
    }
}

#[derive(Clone, Copy, Debug, Error, PartialEq, Eq)]
pub enum RunContextError {
    #[error(transparent)]
    InvalidState(#[from] RunStateError),
    #[error("run deadline cannot be represented by the process-local monotonic clock")]
    DeadlineOutOfRange,
    #[error("trusted wall clock cannot represent the run start")]
    WallClockOutOfRange,
    #[error("run runtime state is unavailable")]
    RuntimeUnavailable,
    #[error("event sequence is exhausted")]
    EventSequenceExhausted,
    #[error("durable runtime transition is invalid")]
    DurableTransitionInvalid,
}
use std::time::{Duration, SystemTime, UNIX_EPOCH};
