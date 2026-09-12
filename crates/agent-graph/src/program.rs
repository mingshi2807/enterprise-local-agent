use std::{future::Future, pin::Pin};

use agent_core::{GraphBranchId, GraphNodeId};
use agent_harness::{DurableApprovalWait, DurableRunState};

use crate::{
    ActionEffects, DecisionContext, GraphProgramError, ModelEffects, RetrieveEffects, VerifyEffects,
};

pub type GraphFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VerificationOutcome {
    Passed,
    Failed,
}

pub trait GraphProgram: Send {
    type WorkingState: Send;

    fn retrieve<'a>(
        &'a mut self,
        node_id: &'a GraphNodeId,
        state: &'a mut Self::WorkingState,
        effects: RetrieveEffects<'a>,
    ) -> GraphFuture<'a, Result<(), GraphProgramError>>;

    fn model<'a>(
        &'a mut self,
        node_id: &'a GraphNodeId,
        state: &'a mut Self::WorkingState,
        effects: ModelEffects<'a>,
    ) -> GraphFuture<'a, Result<(), GraphProgramError>>;

    fn action<'a>(
        &'a mut self,
        node_id: &'a GraphNodeId,
        state: &'a mut Self::WorkingState,
        effects: ActionEffects<'a>,
    ) -> GraphFuture<'a, Result<(), GraphProgramError>>;

    fn durable_local_write_action<'a>(
        &'a mut self,
        _node_id: &'a GraphNodeId,
        _state: &'a mut Self::WorkingState,
        _effects: ActionEffects<'a>,
    ) -> GraphFuture<'a, Result<DurableApprovalWait, GraphProgramError>> {
        Box::pin(async { Err(GraphProgramError::Failed) })
    }

    fn verify<'a>(
        &'a mut self,
        node_id: &'a GraphNodeId,
        state: &'a mut Self::WorkingState,
        effects: VerifyEffects<'a>,
    ) -> GraphFuture<'a, Result<VerificationOutcome, GraphProgramError>>;

    fn decide<'a>(
        &'a mut self,
        node_id: &'a GraphNodeId,
        state: &'a mut Self::WorkingState,
        context: DecisionContext<'a>,
    ) -> GraphFuture<'a, Result<GraphBranchId, GraphProgramError>>;
}

/// Explicit metadata-only reconstruction contract for graph programs.
///
/// Implementations are trusted in-process code. Restoration must perform no
/// external I/O and cannot deserialize evidence, model output, validated
/// actions, tool results, or arbitrary prior working state.
pub trait RestartableGraphProgram: GraphProgram {
    const RECOVERY_VERSION: u32;

    fn restore_working_state(
        &mut self,
        state: &DurableRunState,
    ) -> Result<Self::WorkingState, GraphProgramError>;
}
