use std::time::Duration;

use agent_core::{
    AgentEvent, AgentEventKind, BudgetDimension, BudgetUsage, EventSequence, RunBudget, RunId,
    RunOutcome, RunStateError, RunStatus, SessionId,
};
use serde::{Deserialize, Deserializer, Serialize, de};
use thiserror::Error;

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct RunContext {
    run_id: RunId,
    session_id: SessionId,
    budget: RunBudget,
    usage: BudgetUsage,
    status: RunStatus,
    next_event_sequence: EventSequence,
}

impl RunContext {
    #[must_use]
    pub const fn new(run_id: RunId, session_id: SessionId, budget: RunBudget) -> Self {
        Self {
            run_id,
            session_id,
            budget,
            usage: BudgetUsage::new(0, 0),
            status: RunStatus::Pending,
            next_event_sequence: EventSequence::new(0),
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

    pub fn start(&mut self) -> Result<AgentEvent, RunContextError> {
        let status = self.status.clone().start()?;
        let event = self.next_event(AgentEventKind::RunStatusChanged {
            status: status.clone(),
        })?;
        self.status = status;
        Ok(event)
    }

    pub fn finish(&mut self, outcome: RunOutcome) -> Result<AgentEvent, RunContextError> {
        let status = self.status.clone().finish(outcome.clone())?;
        let event = self.next_event(AgentEventKind::RunFinished { outcome })?;
        self.status = status;
        Ok(event)
    }

    pub fn reserve_model_call(&mut self) -> Result<(), BudgetExceeded> {
        let usage = self.usage.model_calls();
        let limit = self.budget.max_model_calls();
        if usage >= limit {
            return Err(BudgetExceeded::new(BudgetDimension::ModelCalls));
        }

        let reserved = usage + 1;
        self.usage = BudgetUsage::new(reserved, self.usage.tool_calls());
        Ok(())
    }

    pub fn reserve_tool_call(&mut self) -> Result<(), BudgetExceeded> {
        let usage = self.usage.tool_calls();
        let limit = self.budget.max_tool_calls();
        if usage >= limit {
            return Err(BudgetExceeded::new(BudgetDimension::ToolCalls));
        }

        let reserved = usage + 1;
        self.usage = BudgetUsage::new(self.usage.model_calls(), reserved);
        Ok(())
    }

    pub fn check_elapsed(&self, elapsed: Duration) -> Result<(), BudgetExceeded> {
        if elapsed >= self.budget.max_elapsed() {
            return Err(BudgetExceeded::new(BudgetDimension::Elapsed));
        }
        Ok(())
    }

    pub fn next_event(&mut self, kind: AgentEventKind) -> Result<AgentEvent, RunContextError> {
        let sequence = self.next_event_sequence;
        self.next_event_sequence = sequence
            .checked_next()
            .ok_or(RunContextError::EventSequenceExhausted)?;
        Ok(AgentEvent::new(self.run_id, sequence, kind))
    }
}

impl<'de> Deserialize<'de> for RunContext {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct Representation {
            run_id: RunId,
            session_id: SessionId,
            budget: RunBudget,
            usage: BudgetUsage,
            status: RunStatus,
            next_event_sequence: EventSequence,
        }

        let representation = Representation::deserialize(deserializer)?;
        if representation.usage.model_calls() > representation.budget.max_model_calls()
            || representation.usage.tool_calls() > representation.budget.max_tool_calls()
        {
            return Err(de::Error::custom(RunContextError::UsageExceedsBudget));
        }

        Ok(Self {
            run_id: representation.run_id,
            session_id: representation.session_id,
            budget: representation.budget,
            usage: representation.usage,
            status: representation.status,
            next_event_sequence: representation.next_event_sequence,
        })
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
    #[error("event sequence is exhausted")]
    EventSequenceExhausted,
    #[error("serialized budget usage exceeds the configured budget")]
    UsageExceedsBudget,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn context(model_calls: u32, tool_calls: u32) -> RunContext {
        let budget = RunBudget::new(model_calls, tool_calls, Duration::from_secs(10))
            .expect("budget must be valid");
        RunContext::new(RunId::new(), SessionId::new(), budget)
    }

    #[test]
    fn model_budget_is_reserved_before_operation_and_never_overruns() {
        let mut context = context(1, 0);

        context
            .reserve_model_call()
            .expect("first reservation must succeed");
        let denied = context
            .reserve_model_call()
            .expect_err("second reservation must be denied");

        assert_eq!(denied.dimension(), BudgetDimension::ModelCalls);
        assert_eq!(context.usage().model_calls(), 1);
    }

    #[test]
    fn zero_call_limit_denies_without_incrementing() {
        let mut context = context(0, 0);

        assert_eq!(
            context
                .reserve_model_call()
                .expect_err("zero model budget must deny")
                .dimension(),
            BudgetDimension::ModelCalls
        );
        assert_eq!(
            context
                .reserve_tool_call()
                .expect_err("zero tool budget must deny")
                .dimension(),
            BudgetDimension::ToolCalls
        );
        assert_eq!(context.usage(), BudgetUsage::new(0, 0));
    }

    #[test]
    fn tool_budget_is_reserved_before_operation_and_never_overruns() {
        let mut context = context(0, 1);

        context
            .reserve_tool_call()
            .expect("first reservation must succeed");
        let denied = context
            .reserve_tool_call()
            .expect_err("second reservation must be denied");

        assert_eq!(denied.dimension(), BudgetDimension::ToolCalls);
        assert_eq!(context.usage().tool_calls(), 1);
    }

    #[test]
    fn elapsed_limit_denies_at_the_limit_using_supplied_duration() {
        let context = context(0, 0);

        assert!(context.check_elapsed(Duration::from_secs(9)).is_ok());
        assert_eq!(
            context
                .check_elapsed(Duration::from_secs(10))
                .expect_err("elapsed time at limit must be denied")
                .dimension(),
            BudgetDimension::Elapsed
        );
    }

    #[test]
    fn context_emits_deterministically_sequenced_lifecycle_events() {
        let mut context = context(0, 0);

        let started = context.start().expect("run must start");
        let finished = context
            .finish(RunOutcome::Completed)
            .expect("run must finish");

        assert_eq!(started.sequence().get(), 0);
        assert_eq!(finished.sequence().get(), 1);
        assert_eq!(
            context.status(),
            &RunStatus::Finished(RunOutcome::Completed)
        );
    }

    #[test]
    fn context_serialization_preserves_valid_state() {
        let mut context = context(1, 1);
        context.start().expect("run must start");
        context
            .reserve_tool_call()
            .expect("tool reservation must succeed");

        let json = serde_json::to_string(&context).expect("context must serialize");
        let restored: RunContext = serde_json::from_str(&json).expect("context must deserialize");

        assert_eq!(restored, context);
    }
}
