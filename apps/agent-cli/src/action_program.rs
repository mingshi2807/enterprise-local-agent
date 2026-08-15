use agent_core::{LoopFailureKind, ModelRequest, ToolResult};
use agent_harness::{ActionPreparationError, ValidatedAction};
use agent_loop::{
    LoopEffects, LoopFuture, LoopProgram, LoopStepError, ReflectDecision, VerificationResult,
};

pub(crate) struct ActionWorkingState {
    validated_action: Option<ValidatedAction>,
    tool_result: Option<ToolResult>,
}

impl ActionWorkingState {
    #[must_use]
    pub(crate) const fn new() -> Self {
        Self {
            validated_action: None,
            tool_result: None,
        }
    }
}

pub(crate) struct ActionProgram {
    model_request: ModelRequest,
}

impl ActionProgram {
    #[must_use]
    pub(crate) const fn new(model_request: ModelRequest) -> Self {
        Self { model_request }
    }
}

impl LoopProgram for ActionProgram {
    type WorkingState = ActionWorkingState;

    fn observe<'a>(
        &'a mut self,
        _iteration: u32,
        _working_state: &'a mut Self::WorkingState,
        _effects: LoopEffects<'a>,
    ) -> LoopFuture<'a, Result<(), LoopStepError>> {
        Box::pin(std::future::ready(Ok(())))
    }

    fn retrieve<'a>(
        &'a mut self,
        _iteration: u32,
        _working_state: &'a mut Self::WorkingState,
        _effects: LoopEffects<'a>,
    ) -> LoopFuture<'a, Result<(), LoopStepError>> {
        Box::pin(std::future::ready(Ok(())))
    }

    fn plan<'a>(
        &'a mut self,
        _iteration: u32,
        working_state: &'a mut Self::WorkingState,
        mut effects: LoopEffects<'a>,
    ) -> LoopFuture<'a, Result<(), LoopStepError>> {
        let request = self.model_request.clone();
        Box::pin(async move {
            let invocation = effects.invoke_model_tracked(request).await?;
            let action = match effects.prepare_action(invocation).await {
                Ok(action) => action,
                Err(ActionPreparationError::Harness(error)) => return Err(error.into()),
                Err(ActionPreparationError::Validation(_)) => {
                    return Err(LoopStepError::Program);
                }
            };
            working_state.validated_action = Some(action);
            Ok(())
        })
    }

    fn act<'a>(
        &'a mut self,
        _iteration: u32,
        working_state: &'a mut Self::WorkingState,
        mut effects: LoopEffects<'a>,
    ) -> LoopFuture<'a, Result<(), LoopStepError>> {
        Box::pin(async move {
            let action = working_state
                .validated_action
                .take()
                .ok_or(LoopStepError::Program)?;
            working_state.tool_result = Some(effects.invoke_validated_action(action).await?);
            Ok(())
        })
    }

    fn verify<'a>(
        &'a mut self,
        _iteration: u32,
        working_state: &'a mut Self::WorkingState,
        _effects: LoopEffects<'a>,
    ) -> LoopFuture<'a, Result<VerificationResult, LoopStepError>> {
        let verification = match working_state.tool_result {
            Some(ToolResult::Succeeded { .. }) => VerificationResult::Passed,
            Some(ToolResult::DomainFailure { .. }) | None => VerificationResult::Failed,
        };
        Box::pin(std::future::ready(Ok(verification)))
    }

    fn reflect<'a>(
        &'a mut self,
        _iteration: u32,
        _working_state: &'a mut Self::WorkingState,
        verification: VerificationResult,
        _effects: LoopEffects<'a>,
    ) -> LoopFuture<'a, Result<ReflectDecision, LoopStepError>> {
        let decision = match verification {
            VerificationResult::Passed => ReflectDecision::Complete,
            VerificationResult::Failed => ReflectDecision::Fail {
                kind: LoopFailureKind::VerificationFailed,
            },
        };
        Box::pin(std::future::ready(Ok(decision)))
    }
}

#[cfg(test)]
mod tests {
    use std::{sync::Arc, time::Duration};

    use agent_core::{
        AgentEventKind, CapabilityKind, ModelMessage, ModelRequest, ModelRole, RunBudget, RunId,
        RunOutcome, RunStatus, SessionId, ToolDefinition, ToolName, ToolOutput, ToolSchema,
    };
    use agent_harness::{
        AuditFailurePolicy, AuditSink, ExecutionHarness, HarnessConfig, M0ReadOnlyPolicy,
        ModelPort, RunContext, ToolPort, ToolRegistry,
        testing::{FakeToolPort, InMemoryAuditSink},
    };
    use agent_loop::{LoopEngine, TerminalLoopDecision};
    use agent_provider_rig::{RigModelAdapter, testing::FakeRigModel};
    use serde_json::json;

    use super::{ActionProgram, ActionWorkingState};

    #[tokio::test]
    async fn one_iteration_executes_a_model_proposed_read_only_action() {
        let model = FakeRigModel::scripted_text(
            r#"{"action":{"tool":"get_agent_capabilities","arguments":{}}}"#,
        );
        let model_handle = model.clone();
        let model_port: Arc<dyn ModelPort> = Arc::new(RigModelAdapter::new(model));

        let definition = ToolDefinition::new(
            ToolName::new("get_agent_capabilities").expect("tool name must be valid"),
            "report deterministic capabilities",
            CapabilityKind::ReadOnly,
            ToolSchema::new(json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false
            }))
            .expect("tool schema must be valid"),
        )
        .expect("tool definition must be valid");
        let tool = Arc::new(FakeToolPort::succeeding(
            definition,
            ToolOutput::new(json!({"status": "available"})),
        ));
        let tool_handle = Arc::clone(&tool);
        let tool_port: Arc<dyn ToolPort> = tool;
        let mut registry = ToolRegistry::new();
        registry.register(tool_port).expect("tool must register");

        let audit = Arc::new(InMemoryAuditSink::new());
        let audit_sink: Arc<dyn AuditSink> = audit.clone();
        let harness = ExecutionHarness::new(
            model_port,
            registry,
            Arc::new(M0ReadOnlyPolicy),
            audit_sink,
            HarnessConfig::new(AuditFailurePolicy::FailClosed, Duration::from_secs(1))
                .expect("harness config must be valid"),
        );
        let budget = RunBudget::new(1, 1, 1, Duration::from_secs(5)).expect("budget must be valid");
        let mut context = RunContext::new(RunId::new(), SessionId::new(), budget);
        let mut program = ActionProgram::new(ModelRequest::new(vec![ModelMessage::new(
            ModelRole::User,
            "return the strict action envelope",
        )]));
        let mut working_state = ActionWorkingState::new();

        harness
            .start_run(&mut context)
            .await
            .expect("run must start");
        let summary = LoopEngine::new()
            .run(&harness, &mut context, &mut program, &mut working_state)
            .await
            .expect("action loop must complete");

        assert_eq!(summary.terminal_decision(), TerminalLoopDecision::Complete);
        assert_eq!(
            context.status(),
            &RunStatus::Finished(RunOutcome::Completed)
        );
        assert_eq!(context.usage().model_calls(), 1);
        assert_eq!(context.usage().tool_calls(), 1);
        assert_eq!(context.usage().iterations(), 1);
        assert_eq!(model_handle.invocation_count(), 1);
        assert_eq!(tool_handle.invocation_count(), 1);

        let events = audit.events();
        let proposed = events
            .iter()
            .position(|event| matches!(event.kind(), AgentEventKind::ActionProposed { .. }))
            .expect("proposal event must exist");
        let validated = events
            .iter()
            .position(|event| matches!(event.kind(), AgentEventKind::ActionValidated { .. }))
            .expect("validation event must exist");
        let bound = events
            .iter()
            .position(|event| matches!(event.kind(), AgentEventKind::ActionExecutionBound { .. }))
            .expect("binding event must exist");
        let invoked = events
            .iter()
            .position(|event| matches!(event.kind(), AgentEventKind::ToolInvocationStarted { .. }))
            .expect("tool invocation event must exist");
        assert!(proposed < validated && validated < bound && bound < invoked);
    }
}
