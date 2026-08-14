use agent_core::{LoopPhase, RunStatus};
use agent_harness::HarnessError;
use thiserror::Error;

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum LoopStepError {
    #[error("loop phase program failed")]
    Program,
    #[error(transparent)]
    Harness(#[from] HarnessError),
}

#[derive(Clone, Copy, Debug, Error, PartialEq, Eq)]
pub enum LoopTransitionError {
    #[error("loop is already terminal")]
    AlreadyTerminal,
    #[error("loop is not ready to begin an iteration")]
    BeginRequiresReady,
    #[error("phase {phase:?} cannot be entered from current loop position")]
    IllegalPhaseEntry { phase: LoopPhase },
    #[error("phase {phase:?} cannot be completed from current loop position")]
    IllegalPhaseCompletion { phase: LoopPhase },
}

#[derive(Debug, Error)]
pub enum LoopError {
    #[error(transparent)]
    Harness(#[from] HarnessError),
    #[error(transparent)]
    Transition(LoopTransitionError),
    #[error("phase {phase:?} failed")]
    Program { phase: LoopPhase },
    #[error("run became terminal while the loop was executing: {status:?}")]
    RunTerminal { status: RunStatus },
    #[error("the harness committed a terminal state but finalization reported an error")]
    Finalization { source: HarnessError },
}
