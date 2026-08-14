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
}

struct RunRuntime {
    _started_at: Instant,
    deadline_at: Instant,
    cancellation: CancellationToken,
}

impl RunContext {
    #[must_use]
    pub const fn new(run_id: RunId, session_id: SessionId, budget: RunBudget) -> Self {
        Self {
            run_id,
            session_id,
            budget,
            usage: BudgetUsage::new(0, 0, 0),
            status: RunStatus::Pending,
            next_event_sequence: EventSequence::new(0),
            runtime: None,
            audit_degraded: false,
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
    ) -> Result<RunCancellationHandle, RunContextError> {
        let status = self.status.clone().start()?;
        let deadline_at = started_at
            .checked_add(self.budget.max_elapsed())
            .ok_or(RunContextError::DeadlineOutOfRange)?;
        let cancellation = CancellationToken::new();
        let handle = RunCancellationHandle {
            token: cancellation.clone(),
        };

        self.status = status;
        self.runtime = Some(RunRuntime {
            _started_at: started_at,
            deadline_at,
            cancellation,
        });
        Ok(handle)
    }

    pub(crate) fn commit_finished(&mut self, outcome: RunOutcome) -> Result<(), RunContextError> {
        self.status = self.status.clone().finish(outcome)?;
        Ok(())
    }

    pub(crate) fn reserve_model_call(&mut self) -> Result<(), BudgetExceeded> {
        let usage = self.usage.model_calls();
        let limit = self.budget.max_model_calls();
        if usage >= limit {
            return Err(BudgetExceeded::new(BudgetDimension::ModelCalls));
        }

        self.usage = BudgetUsage::new(usage + 1, self.usage.tool_calls(), self.usage.iterations());
        Ok(())
    }

    pub(crate) fn reserve_tool_call(&mut self) -> Result<(), BudgetExceeded> {
        let usage = self.usage.tool_calls();
        let limit = self.budget.max_tool_calls();
        if usage >= limit {
            return Err(BudgetExceeded::new(BudgetDimension::ToolCalls));
        }

        self.usage = BudgetUsage::new(self.usage.model_calls(), usage + 1, self.usage.iterations());
        Ok(())
    }

    pub(crate) fn reserve_iteration(&mut self) -> Result<u32, BudgetExceeded> {
        let usage = self.usage.iterations();
        let limit = self.budget.max_iterations();
        if usage >= limit {
            return Err(BudgetExceeded::new(BudgetDimension::Iterations));
        }

        let iteration = usage + 1;
        self.usage = BudgetUsage::new(self.usage.model_calls(), self.usage.tool_calls(), iteration);
        Ok(iteration)
    }

    pub(crate) fn cancellation(&self) -> Result<CancellationToken, RunContextError> {
        Ok(self.runtime()?.cancellation.clone())
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
    }

    pub(crate) fn next_event(
        &mut self,
        kind: AgentEventKind,
    ) -> Result<AgentEvent, RunContextError> {
        let sequence = self.next_event_sequence;
        self.next_event_sequence = sequence
            .checked_next()
            .ok_or(RunContextError::EventSequenceExhausted)?;
        Ok(AgentEvent::new(self.run_id, sequence, kind))
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
    #[error("run runtime state is unavailable")]
    RuntimeUnavailable,
    #[error("event sequence is exhausted")]
    EventSequenceExhausted,
}
