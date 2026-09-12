use std::marker::PhantomData;

use agent_core::{ModelRequest, ToolResult};
use agent_harness::{
    ActionPreparationError, CompletedKnowledgeRetrieval, CompletedModelInvocation,
    DurableActionContext, DurableApprovalWait, ExecutionHarness, HarnessError, RunContext,
    ValidatedAction,
};
use agent_knowledge::{GroundedModelRequest, KnowledgeRequest};

pub struct RetrieveEffects<'a> {
    harness: &'a ExecutionHarness,
    context: &'a mut RunContext,
}

impl<'a> RetrieveEffects<'a> {
    pub(crate) const fn new(harness: &'a ExecutionHarness, context: &'a mut RunContext) -> Self {
        Self { harness, context }
    }

    pub async fn retrieve_knowledge(
        &mut self,
        request: KnowledgeRequest,
    ) -> Result<CompletedKnowledgeRetrieval, HarnessError> {
        self.harness.retrieve_knowledge(self.context, request).await
    }
}

pub struct ModelEffects<'a> {
    harness: &'a ExecutionHarness,
    context: &'a mut RunContext,
}

impl<'a> ModelEffects<'a> {
    pub(crate) const fn new(harness: &'a ExecutionHarness, context: &'a mut RunContext) -> Self {
        Self { harness, context }
    }

    pub async fn invoke_model(
        &mut self,
        request: ModelRequest,
    ) -> Result<CompletedModelInvocation, HarnessError> {
        self.harness
            .invoke_model_tracked(self.context, request)
            .await
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
}

pub struct ActionEffects<'a> {
    harness: &'a ExecutionHarness,
    context: &'a mut RunContext,
    durable: Option<DurableActionContext>,
}

impl<'a> ActionEffects<'a> {
    pub(crate) const fn new(harness: &'a ExecutionHarness, context: &'a mut RunContext) -> Self {
        Self {
            harness,
            context,
            durable: None,
        }
    }

    pub(crate) const fn new_durable(
        harness: &'a ExecutionHarness,
        context: &'a mut RunContext,
        durable: DurableActionContext,
    ) -> Self {
        Self {
            harness,
            context,
            durable: Some(durable),
        }
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

    pub async fn suspend_local_write(
        &mut self,
        action: ValidatedAction,
    ) -> Result<DurableApprovalWait, HarnessError> {
        let durable = self
            .durable
            .clone()
            .ok_or(HarnessError::DurableApprovalMismatch)?;
        self.harness
            .suspend_durable_local_write(self.context, action, durable)
            .await
    }
}

pub struct VerifyEffects<'a> {
    marker: PhantomData<&'a mut RunContext>,
}

impl VerifyEffects<'_> {
    pub(crate) const fn new() -> Self {
        Self {
            marker: PhantomData,
        }
    }
}

pub struct DecisionContext<'a> {
    marker: PhantomData<&'a RunContext>,
}

impl DecisionContext<'_> {
    pub(crate) const fn new() -> Self {
        Self {
            marker: PhantomData,
        }
    }
}
