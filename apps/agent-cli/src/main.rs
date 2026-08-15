use std::{sync::Arc, time::Duration};

use agent_core::{
    AgentEventKind, CapabilityKind, LoopEventKind, LoopFailureKind, ModelMessage, ModelRequest,
    ModelResponse, ModelRole, RunBudget, RunId, SessionId, ToolCall, ToolCallId, ToolDefinition,
    ToolInput, ToolName, ToolOutput, ToolResult, ToolSchema,
};
use agent_harness::{
    AuditFailurePolicy, AuditSink, ExecutionHarness, HarnessConfig, M0ReadOnlyPolicy, ModelPort,
    RunContext, ToolPort, ToolRegistry,
    testing::{FakeToolPort, InMemoryAuditSink},
};
use agent_loop::{
    LoopEffects, LoopEngine, LoopFuture, LoopProgram, LoopStepError, ReflectDecision,
    VerificationResult,
};
use agent_provider_rig::{RigModelAdapter, testing::FakeRigModel};
use anyhow::Context;

struct DemoWorkingState {
    model_response: Option<ModelResponse>,
    tool_result: Option<ToolResult>,
}

struct DemoProgram {
    model_request: ModelRequest,
    tool_call: ToolCall,
}

impl LoopProgram for DemoProgram {
    type WorkingState = DemoWorkingState;

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
            working_state.model_response = Some(effects.invoke_model(request).await?);
            Ok(())
        })
    }

    fn act<'a>(
        &'a mut self,
        _iteration: u32,
        working_state: &'a mut Self::WorkingState,
        mut effects: LoopEffects<'a>,
    ) -> LoopFuture<'a, Result<(), LoopStepError>> {
        let call = self.tool_call.clone();
        Box::pin(async move {
            working_state.tool_result = Some(effects.invoke_tool(call).await?);
            Ok(())
        })
    }

    fn verify<'a>(
        &'a mut self,
        _iteration: u32,
        working_state: &'a mut Self::WorkingState,
        _effects: LoopEffects<'a>,
    ) -> LoopFuture<'a, Result<VerificationResult, LoopStepError>> {
        let result = if working_state.model_response.is_some()
            && matches!(
                working_state.tool_result,
                Some(ToolResult::Succeeded { .. })
            ) {
            VerificationResult::Passed
        } else {
            VerificationResult::Failed
        };
        Box::pin(std::future::ready(Ok(result)))
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

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_target(false)
        .without_time()
        .try_init()
        .map_err(|error| anyhow::anyhow!("failed to initialize tracing: {error}"))?;

    let budget = RunBudget::new(1, 1, 1, Duration::from_secs(5))
        .context("failed to construct the M3 run budget")?;
    let mut context = RunContext::new(RunId::new(), SessionId::new(), budget);

    let model = Arc::new(RigModelAdapter::new(FakeRigModel::scripted_text(
        "deterministic fake response",
    )));
    let model_port: Arc<dyn ModelPort> = model;

    let tool_call_id = ToolCallId::new();
    let tool_name = ToolName::new("local_lookup")?;
    let definition = ToolDefinition::new(
        tool_name.clone(),
        "deterministic read-only lookup",
        CapabilityKind::ReadOnly,
        ToolSchema::new(serde_json::json!({"type": "object"}))?,
    )?;
    let tool = Arc::new(FakeToolPort::scripted(
        definition,
        vec![Ok(ToolResult::Succeeded {
            call_id: tool_call_id,
            output: ToolOutput::new(serde_json::json!({"status": "available"})),
        })],
    ));
    let tool_port: Arc<dyn ToolPort> = tool;
    let mut tools = ToolRegistry::new();
    tools.register(tool_port)?;

    let audit = Arc::new(InMemoryAuditSink::new());
    let audit_sink: Arc<dyn AuditSink> = audit.clone();
    let config = HarnessConfig::new(AuditFailurePolicy::FailClosed, Duration::from_secs(1))?;
    let harness = ExecutionHarness::new(
        model_port,
        tools,
        Arc::new(M0ReadOnlyPolicy),
        audit_sink,
        config,
    );

    let mut program = DemoProgram {
        model_request: ModelRequest::new(vec![ModelMessage::new(
            ModelRole::User,
            "sentinel prompt stays outside audit events",
        )]),
        tool_call: ToolCall::new(
            tool_call_id,
            tool_name,
            ToolInput::new(serde_json::json!({"sentinel": "tool input"})),
        ),
    };
    let mut working_state = DemoWorkingState {
        model_response: None,
        tool_result: None,
    };

    let _cancellation = harness.start_run(&mut context).await?;
    LoopEngine::new()
        .run(&harness, &mut context, &mut program, &mut working_state)
        .await?;

    println!("RunId: {}", context.run_id());
    println!("Model adapter: RigModelAdapter<FakeRigModel>");
    println!("Run start");
    for event in audit.events() {
        let AgentEventKind::Loop { event } = event.kind() else {
            continue;
        };
        match event {
            LoopEventKind::IterationStarted { iteration, .. } => {
                println!("Iteration {iteration} started");
            }
            LoopEventKind::PhaseEntered { iteration, phase } => {
                println!("Iteration {iteration}: {phase:?} entered");
            }
            LoopEventKind::PhaseCompleted { iteration, phase } => {
                println!("Iteration {iteration}: {phase:?} completed");
            }
            LoopEventKind::ReflectDecision {
                iteration,
                decision,
            } => println!("Iteration {iteration}: Reflect decision {decision:?}"),
            LoopEventKind::IterationCompleted { iteration } => {
                println!("Iteration {iteration} completed");
            }
            LoopEventKind::LoopCompleted {
                completed_iterations,
            } => println!("Loop completed after {completed_iterations} iteration(s)"),
            LoopEventKind::LoopFailed { iteration, kind } => {
                println!("Loop failed in iteration {iteration}: {kind:?}");
            }
        }
    }
    println!("Final status: {:?}", context.status());
    println!("ModelCalls usage: {}", context.usage().model_calls());
    println!("ToolCalls usage: {}", context.usage().tool_calls());
    println!("Iterations usage: {}", context.usage().iterations());
    println!("Audit degraded: {}", context.audit_degraded());

    Ok(())
}
