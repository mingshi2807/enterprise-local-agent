use std::time::Duration;

use serde::{Deserialize, Deserializer, Serialize, de};
use thiserror::Error;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct RunBudget {
    max_model_calls: u32,
    max_tool_calls: u32,
    max_iterations: u32,
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
            max_elapsed,
        })
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
            max_elapsed: Duration,
        }

        let representation = Representation::deserialize(deserializer)?;
        Self::new(
            representation.max_model_calls,
            representation.max_tool_calls,
            representation.max_iterations,
            representation.max_elapsed,
        )
        .map_err(de::Error::custom)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BudgetUsage {
    model_calls: u32,
    tool_calls: u32,
    iterations: u32,
}

impl BudgetUsage {
    #[must_use]
    pub const fn new(model_calls: u32, tool_calls: u32, iterations: u32) -> Self {
        Self {
            model_calls,
            tool_calls,
            iterations,
        }
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
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BudgetDimension {
    ModelCalls,
    ToolCalls,
    Iterations,
    Elapsed,
}

impl std::fmt::Display for BudgetDimension {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let name = match self {
            Self::ModelCalls => "model_calls",
            Self::ToolCalls => "tool_calls",
            Self::Iterations => "iterations",
            Self::Elapsed => "elapsed",
        };
        formatter.write_str(name)
    }
}

#[derive(Clone, Copy, Debug, Error, PartialEq, Eq)]
pub enum BudgetError {
    #[error("max_elapsed must be non-zero")]
    ZeroMaxElapsed,
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
        let usage = BudgetUsage::new(1, 2, 3);

        assert_eq!(usage.model_calls(), 1);
        assert_eq!(usage.tool_calls(), 2);
        assert_eq!(usage.iterations(), 3);
    }

    #[test]
    fn iteration_dimension_display_is_stable() {
        assert_eq!(BudgetDimension::Iterations.to_string(), "iterations");
    }
}
