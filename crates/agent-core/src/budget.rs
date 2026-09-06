use std::time::Duration;

use serde::{Deserialize, Deserializer, Serialize, de};
use thiserror::Error;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct RunBudget {
    max_model_calls: u32,
    max_tool_calls: u32,
    max_iterations: u32,
    max_approval_requests: u32,
    max_graph_steps: u32,
    max_elapsed: Duration,
}

impl RunBudget {
    pub fn new(
        max_model_calls: u32,
        max_tool_calls: u32,
        max_iterations: u32,
        max_elapsed: Duration,
    ) -> Result<Self, BudgetError> {
        if max_elapsed.is_zero() {
            return Err(BudgetError::ZeroMaxElapsed);
        }

        Ok(Self {
            max_model_calls,
            max_tool_calls,
            max_iterations,
            max_approval_requests: 0,
            max_graph_steps: 0,
            max_elapsed,
        })
    }

    #[must_use]
    pub const fn with_max_approval_requests(mut self, max_approval_requests: u32) -> Self {
        self.max_approval_requests = max_approval_requests;
        self
    }

    pub fn with_max_graph_steps(mut self, max_graph_steps: u32) -> Result<Self, BudgetError> {
        if max_graph_steps > crate::MAX_GRAPH_STEPS {
            return Err(BudgetError::GraphStepLimitTooHigh);
        }
        self.max_graph_steps = max_graph_steps;
        Ok(self)
    }

    #[must_use]
    pub const fn max_model_calls(&self) -> u32 {
        self.max_model_calls
    }

    #[must_use]
    pub const fn max_tool_calls(&self) -> u32 {
        self.max_tool_calls
    }

    #[must_use]
    pub const fn max_iterations(&self) -> u32 {
        self.max_iterations
    }

    #[must_use]
    pub const fn max_approval_requests(&self) -> u32 {
        self.max_approval_requests
    }

    #[must_use]
    pub const fn max_graph_steps(&self) -> u32 {
        self.max_graph_steps
    }

    #[must_use]
    pub const fn max_elapsed(&self) -> Duration {
        self.max_elapsed
    }
}

impl<'de> Deserialize<'de> for RunBudget {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct Representation {
            max_model_calls: u32,
            max_tool_calls: u32,
            max_iterations: u32,
            #[serde(default)]
            max_approval_requests: u32,
            #[serde(default)]
            max_graph_steps: u32,
            max_elapsed: Duration,
        }

        let representation = Representation::deserialize(deserializer)?;
        Self::new(
            representation.max_model_calls,
            representation.max_tool_calls,
            representation.max_iterations,
            representation.max_elapsed,
        )
        .map(|budget| budget.with_max_approval_requests(representation.max_approval_requests))
        .and_then(|budget| budget.with_max_graph_steps(representation.max_graph_steps))
        .map_err(de::Error::custom)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BudgetUsage {
    model_calls: u32,
    tool_calls: u32,
    iterations: u32,
    approval_requests: u32,
    graph_steps: u32,
}

impl BudgetUsage {
    #[must_use]
    pub const fn new(model_calls: u32, tool_calls: u32, iterations: u32) -> Self {
        Self {
            model_calls,
            tool_calls,
            iterations,
            approval_requests: 0,
            graph_steps: 0,
        }
    }

    #[must_use]
    pub const fn with_approval_requests(
        model_calls: u32,
        tool_calls: u32,
        iterations: u32,
        approval_requests: u32,
    ) -> Self {
        Self {
            model_calls,
            tool_calls,
            iterations,
            approval_requests,
            graph_steps: 0,
        }
    }

    #[must_use]
    pub const fn with_graph_steps(mut self, graph_steps: u32) -> Self {
        self.graph_steps = graph_steps;
        self
    }

    #[must_use]
    pub const fn model_calls(&self) -> u32 {
        self.model_calls
    }

    #[must_use]
    pub const fn tool_calls(&self) -> u32 {
        self.tool_calls
    }

    #[must_use]
    pub const fn iterations(&self) -> u32 {
        self.iterations
    }

    #[must_use]
    pub const fn approval_requests(&self) -> u32 {
        self.approval_requests
    }

    #[must_use]
    pub const fn graph_steps(&self) -> u32 {
        self.graph_steps
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BudgetDimension {
    ModelCalls,
    ToolCalls,
    Iterations,
    ApprovalRequests,
    GraphSteps,
    Elapsed,
}

impl std::fmt::Display for BudgetDimension {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let name = match self {
            Self::ModelCalls => "model_calls",
            Self::ToolCalls => "tool_calls",
            Self::Iterations => "iterations",
            Self::ApprovalRequests => "approval_requests",
            Self::GraphSteps => "graph_steps",
            Self::Elapsed => "elapsed",
        };
        formatter.write_str(name)
    }
}

#[derive(Clone, Copy, Debug, Error, PartialEq, Eq)]
pub enum BudgetError {
    #[error("max_elapsed must be non-zero")]
    ZeroMaxElapsed,
    #[error("max_graph_steps exceeds the implementation ceiling")]
    GraphStepLimitTooHigh,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_call_limits_are_valid() {
        let budget = RunBudget::new(0, 0, 1, Duration::from_secs(1)).expect("budget must be valid");

        assert_eq!(budget.max_model_calls(), 0);
        assert_eq!(budget.max_tool_calls(), 0);
        assert_eq!(budget.max_iterations(), 1);
        assert_eq!(budget.max_approval_requests(), 0);
    }

    #[test]
    fn zero_iteration_limit_is_valid() {
        let budget = RunBudget::new(1, 1, 0, Duration::from_secs(1)).expect("budget must be valid");

        assert_eq!(budget.max_iterations(), 0);
    }

    #[test]
    fn zero_elapsed_limit_is_rejected() {
        assert_eq!(
            RunBudget::new(0, 0, 0, Duration::ZERO),
            Err(BudgetError::ZeroMaxElapsed)
        );
    }

    #[test]
    fn invalid_serialized_budget_is_rejected() {
        let json = r#"{"max_model_calls":1,"max_tool_calls":1,"max_iterations":1,"max_elapsed":{"secs":0,"nanos":0}}"#;

        let error = serde_json::from_str::<RunBudget>(json).expect_err("zero duration must fail");

        assert!(error.to_string().contains("max_elapsed must be non-zero"));
    }

    #[test]
    fn budget_round_trips_through_json() {
        let budget =
            RunBudget::new(2, 3, 4, Duration::from_millis(750)).expect("budget must be valid");

        let json = serde_json::to_string(&budget).expect("budget must serialize");
        let restored: RunBudget = serde_json::from_str(&json).expect("budget must deserialize");

        assert_eq!(restored, budget);
    }

    #[test]
    fn budget_usage_tracks_iterations() {
        let usage = BudgetUsage::with_approval_requests(1, 2, 3, 4);

        assert_eq!(usage.model_calls(), 1);
        assert_eq!(usage.tool_calls(), 2);
        assert_eq!(usage.iterations(), 3);
        assert_eq!(usage.approval_requests(), 4);
    }

    #[test]
    fn iteration_dimension_display_is_stable() {
        assert_eq!(BudgetDimension::Iterations.to_string(), "iterations");
    }

    #[test]
    fn approval_budget_defaults_to_zero_and_uses_named_builder() {
        let budget = RunBudget::new(1, 1, 1, Duration::from_secs(1)).expect("budget must be valid");

        assert_eq!(budget.max_approval_requests(), 0);
        assert_eq!(
            budget.with_max_approval_requests(2).max_approval_requests(),
            2
        );
    }

    #[test]
    fn approval_dimension_display_is_stable() {
        assert_eq!(
            BudgetDimension::ApprovalRequests.to_string(),
            "approval_requests"
        );
    }

    #[test]
    fn graph_budget_is_additive_bounded_and_defaults_to_disabled() {
        let budget = RunBudget::new(1, 1, 1, Duration::from_secs(1)).expect("budget");
        assert_eq!(budget.max_graph_steps(), 0);
        assert_eq!(
            budget
                .with_max_graph_steps(crate::MAX_GRAPH_STEPS)
                .expect("ceiling")
                .max_graph_steps(),
            crate::MAX_GRAPH_STEPS
        );
        assert_eq!(
            budget.with_max_graph_steps(crate::MAX_GRAPH_STEPS + 1),
            Err(BudgetError::GraphStepLimitTooHigh)
        );
    }

    #[test]
    fn graph_usage_and_dimension_are_independent_from_iterations() {
        let usage = BudgetUsage::new(1, 2, 3).with_graph_steps(4);
        assert_eq!(usage.iterations(), 3);
        assert_eq!(usage.graph_steps(), 4);
        assert_eq!(BudgetDimension::GraphSteps.to_string(), "graph_steps");
    }
}
