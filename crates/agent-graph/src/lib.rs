//! Deterministic, provider-neutral orchestration for bounded workflow DAGs.

mod definition;
mod effects;
mod engine;
mod error;
mod program;

pub use agent_core::{
    GraphBranchId, GraphNodeId, GraphNodeKind, GraphRecoveryMode, GraphTransitionKey,
};
pub use definition::{
    Edge, GraphDefinition, GraphDefinitionError, MAX_GRAPH_EDGES, MAX_GRAPH_NODES, NodeDefinition,
    NodeKind,
};
pub use effects::{ActionEffects, DecisionContext, ModelEffects, RetrieveEffects, VerifyEffects};
pub use engine::{GraphEngine, GraphRunSummary, GraphTerminalOutcome};
pub use error::{GraphError, GraphProgramError};
pub use program::{GraphFuture, GraphProgram, RestartableGraphProgram, VerificationOutcome};

#[cfg(test)]
mod tests;
