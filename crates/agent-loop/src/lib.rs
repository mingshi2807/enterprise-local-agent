//! Deterministic outer-loop control for the enterprise agent runtime.
//!
//! `LoopProgram` implementations are trusted in-process application logic. M2
//! requires enterprise effects to use `LoopEffects`, but Rust cannot prevent a
//! downstream trusted implementation from doing unrelated I/O on its own.
//! Untrusted plugin execution and sandbox enforcement are future scope.
//!
//! `LoopState` is deliberately not serializable in M2. Durable execution must
//! use a future, explicitly versioned snapshot contract rather than treating
//! process-local control state as a checkpoint.
//!
//! ```compile_fail
//! let state = agent_loop::LoopState::new();
//! let _ = serde_json::to_string(&state);
//! ```

mod engine;
mod error;
mod program;
mod state;

pub use engine::{LoopEngine, LoopRunSummary, TerminalLoopDecision};
pub use error::{LoopError, LoopStepError, LoopTransitionError};
pub use program::{
    LoopEffects, LoopFuture, LoopProgram, ReflectDecision, RestartableLoopProgram,
    VerificationResult,
};
pub use state::{LoopPosition, LoopState, LoopTerminal};

#[cfg(test)]
mod engine_tests;
#[cfg(test)]
mod knowledge_tests;
