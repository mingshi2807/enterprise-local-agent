use std::{future::Future, pin::Pin};

use agent_core::{LoopFailureKind, ModelRequest, ModelResponse, ToolCall, ToolResult};
use agent_harness::{
    ActionPreparationError, CompletedKnowledgeRetrieval, CompletedModelInvocation,
    ExecutionHarness, HarnessError, RunContext, ValidatedAction,
};
use agent_knowledge::{GroundedModelRequest, KnowledgeRequest};

use crate::LoopStepError;

pub type LoopFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VerificationResult {
    Passed,
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReflectDecision {
    Complete,
    Continue,
    Fail { kind: LoopFailureKind },
}

pub trait LoopProgram: Send {
    type WorkingState: Send;

    fn observe<'a>(
        &'a mut self,
        iteration: u32,
        working_state: &'a mut Self::WorkingState,
        effects: LoopEffects<'a>,
    ) -> LoopFuture<'a, Result<(), LoopStepError>>;

    fn retrieve<'a>(
        &'a mut self,
        iteration: u32,
        working_state: &'a mut Self::WorkingState,
        effects: LoopEffects<'a>,
    ) -> LoopFuture<'a, Result<(), LoopStepError>>;

    fn plan<'a>(
        &'a mut self,
        iteration: u32,
        working_state: &'a mut Self::WorkingState,
        effects: LoopEffects<'a>,
    ) -> LoopFuture<'a, Result<(), LoopStepError>>;

    fn act<'a>(
        &'a mut self,
        iteration: u32,
        working_state: &'a mut Self::WorkingState,
        effects: LoopEffects<'a>,
    ) -> LoopFuture<'a, Result<(), LoopStepError>>;

    fn verify<'a>(
        &'a mut self,
        iteration: u32,
        working_state: &'a mut Self::WorkingState,
        effects: LoopEffects<'a>,
    ) -> LoopFuture<'a, Result<VerificationResult, LoopStepError>>;

    fn reflect<'a>(
        &'a mut self,
        iteration: u32,
        working_state: &'a mut Self::WorkingState,
        verification: VerificationResult,
        effects: LoopEffects<'a>,
    ) -> LoopFuture<'a, Result<ReflectDecision, LoopStepError>>;
}

/// A loop program that can rebuild its working state from metadata-only M7 state.
///
/// Implementations are trusted to perform no I/O or external effects while
/// restoring. Arbitrary `LoopProgram::WorkingState` is never deserialized.
pub trait RestartableLoopProgram: LoopProgram {
    const RECOVERY_VERSION: u32;
    const RESTART_INTERRUPTED_RETRIEVAL: bool = false;

    fn restore_working_state(
        &mut self,
        state: &agent_harness::DurableRunState,
    ) -> Result<Self::WorkingState, LoopStepError>;
}

/// The only enterprise effect surface supplied to a loop program.
///
/// `LoopProgram` is trusted in-process application logic. This type preserves
/// the project boundary for model and tool operations, but it is not a sandbox
/// and cannot prevent a downstream implementation from performing unrelated
/// I/O through other APIs it chooses to depend on.
pub struct LoopEffects<'a> {
    pub(crate) harness: &'a ExecutionHarness,
    pub(crate) context: &'a mut RunContext,
}

impl<'a> LoopEffects<'a> {
    pub(crate) fn new(harness: &'a ExecutionHarness, context: &'a mut RunContext) -> Self {
        Self { harness, context }
    }

    pub async fn invoke_model(
        &mut self,
        request: ModelRequest,
    ) -> Result<ModelResponse, HarnessError> {
        self.harness.invoke_model(self.context, request).await
    }

    pub async fn invoke_model_tracked(
        &mut self,
        request: ModelRequest,
    ) -> Result<CompletedModelInvocation, HarnessError> {
        self.harness
            .invoke_model_tracked(self.context, request)
            .await
    }

    pub async fn retrieve_knowledge(
        &mut self,
        request: KnowledgeRequest,
    ) -> Result<CompletedKnowledgeRetrieval, HarnessError> {
        self.harness.retrieve_knowledge(self.context, request).await
    }

    pub async fn invoke_grounded_model(
        &mut self,
        retrieval: &CompletedKnowledgeRetrieval,
        request: GroundedModelRequest,
    ) -> Result<CompletedModelInvocation, HarnessError> {
        self.harness
            .invoke_grounded_model(self.context, retrieval, request)
            .await
    }

    pub async fn prepare_action(
        &mut self,
        invocation: CompletedModelInvocation,
    ) -> Result<ValidatedAction, ActionPreparationError> {
        self.harness.prepare_action(self.context, invocation).await
    }

    pub async fn invoke_validated_action(
        &mut self,
        action: ValidatedAction,
    ) -> Result<ToolResult, HarnessError> {
        self.harness
            .invoke_validated_action(self.context, action)
            .await
    }

    pub async fn invoke_tool(&mut self, call: ToolCall) -> Result<ToolResult, HarnessError> {
        self.harness.invoke_tool(self.context, call).await
    }
}
