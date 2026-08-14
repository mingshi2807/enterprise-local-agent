use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{BudgetDimension, LoopFailureKind};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Pending,
    Running,
    Finished(RunOutcome),
}

impl RunStatus {
    pub fn start(self) -> Result<Self, RunStateError> {
        match self {
            Self::Pending => Ok(Self::Running),
            Self::Running | Self::Finished(_) => Err(RunStateError::StartRequiresPending),
        }
    }

    pub fn finish(self, outcome: RunOutcome) -> Result<Self, RunStateError> {
        match self {
            Self::Running => Ok(Self::Finished(outcome)),
            Self::Pending | Self::Finished(_) => Err(RunStateError::FinishRequiresRunning),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunOutcome {
    Completed,
    Cancelled,
    BudgetExceeded { dimension: BudgetDimension },
    Failed { kind: RunFailureKind },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunFailureKind {
    Model,
    Tool,
    AuditUnavailable,
    Loop { kind: LoopFailureKind },
    Internal,
}

#[derive(Clone, Copy, Debug, Error, PartialEq, Eq)]
pub enum RunStateError {
    #[error("a run can start only from pending state")]
    StartRequiresPending,
    #[error("a run can finish only from running state")]
    FinishRequiresRunning,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_machine_has_one_terminal_representation() {
        let status = RunStatus::Pending
            .start()
            .expect("pending run must start")
            .finish(RunOutcome::Completed)
            .expect("running run must finish");

        assert_eq!(status, RunStatus::Finished(RunOutcome::Completed));
    }

    #[test]
    fn terminal_run_cannot_start_or_finish_again() {
        let terminal = RunStatus::Finished(RunOutcome::Cancelled);

        assert_eq!(
            terminal.clone().start(),
            Err(RunStateError::StartRequiresPending)
        );
        assert_eq!(
            terminal.finish(RunOutcome::Completed),
            Err(RunStateError::FinishRequiresRunning)
        );
    }
}
