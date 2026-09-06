use thiserror::Error;

use crate::GraphDefinitionError;

#[derive(Clone, Copy, Debug, Error, PartialEq, Eq)]
pub enum GraphProgramError {
    #[error("graph callback failed")]
    Failed,
}

#[derive(Debug, Error)]
pub enum GraphError {
    #[error(transparent)]
    Definition(#[from] GraphDefinitionError),
    #[error(transparent)]
    Harness(#[from] agent_harness::HarnessError),
    #[error(transparent)]
    Program(#[from] GraphProgramError),
    #[error("graph recovery contract or durable position does not match")]
    RecoveryMismatch,
    #[error("validated graph transition is unavailable")]
    TransitionUnavailable,
}
