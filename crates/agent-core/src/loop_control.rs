use serde::{Deserialize, Serialize};

use crate::RunFailureKind;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LoopPhase {
    Observe,
    Retrieve,
    Plan,
    Act,
    Verify,
    Reflect,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LoopFailureKind {
    Program { phase: LoopPhase },
    VerificationFailed,
    InvariantViolation,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LoopDecisionKind {
    Complete,
    Continue,
    Fail,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LoopEventKind {
    IterationStarted {
        iteration: u32,
        usage: u32,
        limit: u32,
    },
    PhaseEntered {
        iteration: u32,
        phase: LoopPhase,
    },
    PhaseCompleted {
        iteration: u32,
        phase: LoopPhase,
    },
    ReflectDecision {
        iteration: u32,
        decision: LoopDecisionKind,
    },
    IterationCompleted {
        iteration: u32,
    },
    LoopCompleted {
        completed_iterations: u32,
    },
    LoopFailed {
        iteration: u32,
        kind: RunFailureKind,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoopProgressEvent {
    PhaseEntered {
        iteration: u32,
        phase: LoopPhase,
    },
    PhaseCompleted {
        iteration: u32,
        phase: LoopPhase,
    },
    ReflectDecision {
        iteration: u32,
        decision: LoopDecisionKind,
    },
    IterationCompleted {
        iteration: u32,
    },
    LoopCompleted {
        completed_iterations: u32,
    },
    LoopFailed {
        iteration: u32,
        kind: RunFailureKind,
    },
}

impl From<LoopProgressEvent> for LoopEventKind {
    fn from(event: LoopProgressEvent) -> Self {
        match event {
            LoopProgressEvent::PhaseEntered { iteration, phase } => {
                Self::PhaseEntered { iteration, phase }
            }
            LoopProgressEvent::PhaseCompleted { iteration, phase } => {
                Self::PhaseCompleted { iteration, phase }
            }
            LoopProgressEvent::ReflectDecision {
                iteration,
                decision,
            } => Self::ReflectDecision {
                iteration,
                decision,
            },
            LoopProgressEvent::IterationCompleted { iteration } => {
                Self::IterationCompleted { iteration }
            }
            LoopProgressEvent::LoopCompleted {
                completed_iterations,
            } => Self::LoopCompleted {
                completed_iterations,
            },
            LoopProgressEvent::LoopFailed { iteration, kind } => {
                Self::LoopFailed { iteration, kind }
            }
        }
    }
}
