use std::{sync::Arc, time::Duration};

use agent_core::{
    CapabilityKind, ModelMessage, ModelOutputPart, ModelRequest, ModelResponse, ModelRole,
    RunBudget, RunId, SessionId, ToolCall, ToolCallId, ToolDefinition, ToolInput, ToolName,
    ToolOutput, ToolResult, ToolSchema,
};
use agent_harness::{
    AuditFailurePolicy, AuditSink, ExecutionHarness, HarnessConfig, M0ReadOnlyPolicy, ModelPort,
    RunContext, ToolPort, ToolRegistry,
    testing::{FakeModelPort, FakeToolPort, InMemoryAuditSink},
};
use anyhow::Context;

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_target(false)
        .without_time()
        .try_init()
        .map_err(|error| anyhow::anyhow!("failed to initialize tracing: {error}"))?;

    let budget = RunBudget::new(1, 1, Duration::from_secs(5))
        .context("failed to construct the M1 run budget")?;
    let mut context = RunContext::new(RunId::new(), SessionId::new(), budget);

    let model = Arc::new(FakeModelPort::scripted(vec![Ok(ModelResponse::new(
        vec![ModelOutputPart::Text(
            "deterministic fake response".to_owned(),
        )],
        None,
    ))]));
    let model_port: Arc<dyn ModelPort> = model;

    let tool_name = ToolName::new("local_lookup")?;
    let tool_call_id = ToolCallId::new();
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
    let audit_sink: Arc<dyn AuditSink> = audit;
    let config = HarnessConfig::new(AuditFailurePolicy::FailClosed, Duration::from_secs(1))?;
    let harness = ExecutionHarness::new(
        model_port,
        tools,
        Arc::new(M0ReadOnlyPolicy),
        audit_sink,
        config,
    );

    let _cancellation = harness.start_run(&mut context).await?;
    harness
        .invoke_model(
            &mut context,
            ModelRequest::new(vec![ModelMessage::new(
                ModelRole::User,
                "sentinel prompt stays outside audit events",
            )]),
        )
        .await?;
    harness
        .invoke_tool(
            &mut context,
            ToolCall::new(
                tool_call_id,
                tool_name,
                ToolInput::new(serde_json::json!({"sentinel": "tool input"})),
            ),
        )
        .await?;
    harness.complete_run(&mut context).await?;

    println!("RunId: {}", context.run_id());
    println!("Final status: {:?}", context.status());
    println!(
        "Budget usage: model_calls={}, tool_calls={}",
        context.usage().model_calls(),
        context.usage().tool_calls()
    );

    Ok(())
}
