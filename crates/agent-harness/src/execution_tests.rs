use std::{sync::Arc, time::Duration};

use agent_core::{
    AgentEventKind, BudgetDimension, CapabilityKind, LoopEventKind, LoopPhase, LoopProgressEvent,
    ModelMessage, ModelOutputPart, ModelRequest, ModelResponse, ModelRole, RunBudget,
    RunFailureKind, RunId, RunOutcome, RunStatus, SessionId, ToolCall, ToolCallId, ToolDefinition,
    ToolDomainFailure, ToolDomainFailureKind, ToolInput, ToolName, ToolOutput, ToolResult,
    ToolSchema,
};

use crate::{
    AuditFailurePolicy, AuditPhase, AuditSink, ExecutionHarness, ExecutionStage, HarnessConfig,
    HarnessError, M0ReadOnlyPolicy, ModelPort, ModelPortError, OperationEffect, RunContext,
    ToolPort, ToolPortError, ToolRegistry,
    testing::{
        FailingAuditSink, FakeInvocation, FakeModelPort, FakeToolPort, InMemoryAuditSink,
        InvocationLog,
    },
};

fn budget(model_calls: u32, tool_calls: u32, elapsed: Duration) -> RunBudget {
    RunBudget::new(model_calls, tool_calls, 1, elapsed).expect("test budget must be valid")
}

fn budget_with_iterations(
    model_calls: u32,
    tool_calls: u32,
    iterations: u32,
    elapsed: Duration,
) -> RunBudget {
    RunBudget::new(model_calls, tool_calls, iterations, elapsed).expect("test budget must be valid")
}

fn context(model_calls: u32, tool_calls: u32, elapsed: Duration) -> RunContext {
    RunContext::new(
        RunId::new(),
        SessionId::new(),
        budget(model_calls, tool_calls, elapsed),
    )
}

fn context_with_iterations(
    model_calls: u32,
    tool_calls: u32,
    iterations: u32,
    elapsed: Duration,
) -> RunContext {
    RunContext::new(
        RunId::new(),
        SessionId::new(),
        budget_with_iterations(model_calls, tool_calls, iterations, elapsed),
    )
}

fn request(content: &str) -> ModelRequest {
    ModelRequest::new(vec![ModelMessage::new(ModelRole::User, content)])
}

fn response(content: &str) -> ModelResponse {
    ModelResponse::new(vec![ModelOutputPart::Text(content.to_owned())], None)
}

fn definition(name: &str, capability: CapabilityKind) -> ToolDefinition {
    ToolDefinition::new(
        ToolName::new(name).expect("test tool name must be valid"),
        "test tool",
        capability,
        ToolSchema::new(serde_json::json!({"type": "object"})).expect("test schema must be valid"),
    )
    .expect("test definition must be valid")
}

fn call(name: &str, id: ToolCallId, sentinel: &str) -> ToolCall {
    ToolCall::new(
        id,
        ToolName::new(name).expect("test tool name must be valid"),
        ToolInput::new(serde_json::json!({"value": sentinel})),
    )
}

fn config(policy: AuditFailurePolicy) -> HarnessConfig {
    HarnessConfig::new(policy, Duration::from_secs(1)).expect("test config must be valid")
}

fn harness_error<T>(result: Result<T, HarnessError>, message: &str) -> HarnessError {
    match result {
        Ok(_) => panic!("{message}"),
        Err(error) => error,
    }
}

fn harness(
    model: Arc<FakeModelPort>,
    tools: Vec<Arc<FakeToolPort>>,
    audit: Arc<dyn AuditSink>,
    policy: AuditFailurePolicy,
) -> ExecutionHarness {
    let model: Arc<dyn ModelPort> = model;
    let mut registry = ToolRegistry::new();
    for tool in tools {
        let port: Arc<dyn ToolPort> = tool;
        registry.register(port).expect("test tool must register");
    }
    ExecutionHarness::new(
        model,
        registry,
        Arc::new(M0ReadOnlyPolicy),
        audit,
        config(policy),
    )
}

#[tokio::test]
async fn model_budget_is_reserved_and_audited_before_invocation() {
    let log = InvocationLog::default();
    let model = Arc::new(FakeModelPort::with_log(
        vec![Ok(response("fake output"))],
        log.clone(),
    ));
    let audit = Arc::new(InMemoryAuditSink::with_log(log.clone()));
    let runtime = harness(
        model.clone(),
        vec![],
        audit.clone(),
        AuditFailurePolicy::FailClosed,
    );
    let mut run = context(1, 0, Duration::from_secs(30));

    runtime.start_run(&mut run).await.expect("run must start");
    runtime
        .invoke_model(&mut run, request("prompt"))
        .await
        .expect("model invocation must succeed");

    assert_eq!(run.usage().model_calls(), 1);
    assert_eq!(model.invocation_count(), 1);
    assert_eq!(
        log.entries(),
        vec![
            FakeInvocation::Audit,
            FakeInvocation::Audit,
            FakeInvocation::Model,
            FakeInvocation::Audit,
        ]
    );
    assert!(matches!(
        audit.events()[1].kind(),
        AgentEventKind::ModelInvocationStarted { usage: 1, .. }
    ));
}

#[tokio::test]
async fn zero_model_budget_never_invokes_model_port() {
    let model = Arc::new(FakeModelPort::scripted(vec![Ok(response("unused"))]));
    let audit = Arc::new(InMemoryAuditSink::new());
    let runtime = harness(model.clone(), vec![], audit, AuditFailurePolicy::FailClosed);
    let mut run = context(0, 0, Duration::from_secs(30));
    runtime.start_run(&mut run).await.expect("run must start");

    let error = harness_error(
        runtime.invoke_model(&mut run, request("prompt")).await,
        "zero budget must deny invocation",
    );

    assert_eq!(error.budget_dimension(), Some(BudgetDimension::ModelCalls));
    assert_eq!(model.invocation_count(), 0);
    assert_eq!(
        run.status(),
        &RunStatus::Finished(RunOutcome::BudgetExceeded {
            dimension: BudgetDimension::ModelCalls,
        })
    );
    assert!(matches!(
        runtime.invoke_model(&mut run, request("later")).await,
        Err(HarnessError::InvalidLifecycle { .. })
    ));
}

#[tokio::test]
async fn exhausted_model_budget_terminalizes_and_prevents_later_operations() {
    let model = Arc::new(FakeModelPort::scripted(vec![
        Ok(response("first")),
        Ok(response("must not execute")),
    ]));
    let runtime = harness(
        model.clone(),
        vec![],
        Arc::new(InMemoryAuditSink::new()),
        AuditFailurePolicy::FailClosed,
    );
    let mut run = context(1, 0, Duration::from_secs(30));
    runtime.start_run(&mut run).await.expect("run must start");
    runtime
        .invoke_model(&mut run, request("first"))
        .await
        .expect("first reserved invocation must succeed");

    let error = harness_error(
        runtime.invoke_model(&mut run, request("second")).await,
        "second reservation must encounter the configured limit",
    );

    assert_eq!(error.budget_dimension(), Some(BudgetDimension::ModelCalls));
    assert_eq!(model.invocation_count(), 1);
    assert_eq!(
        run.status(),
        &RunStatus::Finished(RunOutcome::BudgetExceeded {
            dimension: BudgetDimension::ModelCalls,
        })
    );
    assert!(matches!(
        runtime.complete_run(&mut run).await,
        Err(HarnessError::InvalidLifecycle { .. })
    ));
}

#[tokio::test]
async fn read_only_tool_executes_and_uses_correlated_call_id() {
    let call_id = ToolCallId::new();
    let tool = Arc::new(FakeToolPort::scripted(
        definition("lookup", CapabilityKind::ReadOnly),
        vec![Ok(ToolResult::Succeeded {
            call_id,
            output: ToolOutput::new(serde_json::json!({"ok": true})),
        })],
    ));
    let audit = Arc::new(InMemoryAuditSink::new());
    let model = Arc::new(FakeModelPort::scripted(vec![]));
    let runtime = harness(
        model,
        vec![tool.clone()],
        audit.clone(),
        AuditFailurePolicy::FailClosed,
    );
    let mut run = context(0, 1, Duration::from_secs(30));
    runtime.start_run(&mut run).await.expect("run must start");

    runtime
        .invoke_tool(&mut run, call("lookup", call_id, "input"))
        .await
        .expect("read-only tool must execute");

    assert_eq!(tool.invocation_count(), 1);
    assert_eq!(run.usage().tool_calls(), 1);
    let tool_ids: Vec<_> = audit
        .events()
        .into_iter()
        .filter_map(|event| match event.kind() {
            AgentEventKind::ToolInvocationStarted { tool_call_id, .. }
            | AgentEventKind::ToolInvocationCompleted { tool_call_id } => Some(*tool_call_id),
            _ => None,
        })
        .collect();
    assert_eq!(tool_ids, vec![call_id, call_id]);
}

#[tokio::test]
async fn denied_local_write_is_not_invoked_and_consumes_no_tool_budget() {
    let call_id = ToolCallId::new();
    let tool = Arc::new(FakeToolPort::scripted(
        definition("write", CapabilityKind::LocalWrite),
        vec![Ok(ToolResult::Succeeded {
            call_id,
            output: ToolOutput::new(serde_json::json!({})),
        })],
    ));
    let runtime = harness(
        Arc::new(FakeModelPort::scripted(vec![])),
        vec![tool.clone()],
        Arc::new(InMemoryAuditSink::new()),
        AuditFailurePolicy::FailClosed,
    );
    let mut run = context(0, 1, Duration::from_secs(30));
    runtime.start_run(&mut run).await.expect("run must start");

    let error = runtime
        .invoke_tool(&mut run, call("write", call_id, "input"))
        .await
        .expect_err("local write must be denied");

    assert!(matches!(error, HarnessError::PolicyDenied(_)));
    assert_eq!(tool.invocation_count(), 0);
    assert_eq!(run.usage().tool_calls(), 0);
    assert_eq!(run.status(), &RunStatus::Running);
}

#[tokio::test]
async fn exhausted_tool_budget_prevents_invocation() {
    let tool = Arc::new(FakeToolPort::scripted(
        definition("lookup", CapabilityKind::ReadOnly),
        vec![],
    ));
    let runtime = harness(
        Arc::new(FakeModelPort::scripted(vec![])),
        vec![tool.clone()],
        Arc::new(InMemoryAuditSink::new()),
        AuditFailurePolicy::FailClosed,
    );
    let mut run = context(0, 0, Duration::from_secs(30));
    runtime.start_run(&mut run).await.expect("run must start");

    let error = runtime
        .invoke_tool(&mut run, call("lookup", ToolCallId::new(), "input"))
        .await
        .expect_err("zero tool budget must deny invocation");

    assert_eq!(error.budget_dimension(), Some(BudgetDimension::ToolCalls));
    assert_eq!(tool.invocation_count(), 0);
    assert_eq!(
        run.status(),
        &RunStatus::Finished(RunOutcome::BudgetExceeded {
            dimension: BudgetDimension::ToolCalls,
        })
    );
    assert!(matches!(
        runtime.complete_run(&mut run).await,
        Err(HarnessError::InvalidLifecycle { .. })
    ));
}

#[tokio::test]
async fn cancellation_before_invocation_terminalizes_run() {
    let model = Arc::new(FakeModelPort::scripted(vec![Ok(response("unused"))]));
    let runtime = harness(
        model.clone(),
        vec![],
        Arc::new(InMemoryAuditSink::new()),
        AuditFailurePolicy::FailClosed,
    );
    let mut run = context(1, 0, Duration::from_secs(30));
    let cancellation = runtime.start_run(&mut run).await.expect("run must start");
    cancellation.request_cancel();

    let error = harness_error(
        runtime.invoke_model(&mut run, request("prompt")).await,
        "cancelled run must reject invocation",
    );

    assert!(matches!(
        error,
        HarnessError::Cancelled {
            stage: ExecutionStage::Preflight
        }
    ));
    assert_eq!(run.status(), &RunStatus::Finished(RunOutcome::Cancelled));
    assert_eq!(model.invocation_count(), 0);
}

#[tokio::test(start_paused = true)]
async fn cancellation_during_invocation_terminalizes_run() {
    let model = Arc::new(FakeModelPort::pending());
    let runtime = harness(
        model.clone(),
        vec![],
        Arc::new(InMemoryAuditSink::new()),
        AuditFailurePolicy::FailClosed,
    );
    let mut run = context(1, 0, Duration::from_secs(30));
    let cancellation = runtime.start_run(&mut run).await.expect("run must start");
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(1)).await;
        cancellation.request_cancel();
    });

    let error = harness_error(
        runtime.invoke_model(&mut run, request("prompt")).await,
        "in-flight invocation must observe cancellation",
    );

    assert!(matches!(
        error,
        HarnessError::Cancelled {
            stage: ExecutionStage::Invocation
        }
    ));
    assert_eq!(run.status(), &RunStatus::Finished(RunOutcome::Cancelled));
    assert_eq!(model.invocation_count(), 1);
}

#[tokio::test(start_paused = true)]
async fn deadline_before_invocation_terminalizes_run() {
    let model = Arc::new(FakeModelPort::scripted(vec![Ok(response("unused"))]));
    let runtime = harness(
        model.clone(),
        vec![],
        Arc::new(InMemoryAuditSink::new()),
        AuditFailurePolicy::FailClosed,
    );
    let mut run = context(1, 0, Duration::from_secs(1));
    runtime.start_run(&mut run).await.expect("run must start");
    tokio::time::advance(Duration::from_secs(1)).await;

    let error = harness_error(
        runtime.invoke_model(&mut run, request("prompt")).await,
        "expired deadline must reject invocation",
    );

    assert!(matches!(
        error,
        HarnessError::DeadlineExceeded {
            stage: ExecutionStage::Preflight
        }
    ));
    assert_eq!(
        run.status(),
        &RunStatus::Finished(RunOutcome::BudgetExceeded {
            dimension: BudgetDimension::Elapsed,
        })
    );
    assert_eq!(model.invocation_count(), 0);
}

#[tokio::test(start_paused = true)]
async fn deadline_during_invocation_terminalizes_run() {
    let model = Arc::new(FakeModelPort::pending());
    let runtime = harness(
        model.clone(),
        vec![],
        Arc::new(InMemoryAuditSink::new()),
        AuditFailurePolicy::FailClosed,
    );
    let mut run = context(1, 0, Duration::from_secs(1));
    runtime.start_run(&mut run).await.expect("run must start");

    let error = harness_error(
        runtime.invoke_model(&mut run, request("prompt")).await,
        "in-flight invocation must observe deadline",
    );

    assert!(matches!(
        error,
        HarnessError::DeadlineExceeded {
            stage: ExecutionStage::Invocation
        }
    ));
    assert_eq!(
        run.status(),
        &RunStatus::Finished(RunOutcome::BudgetExceeded {
            dimension: BudgetDimension::Elapsed,
        })
    );
    assert_eq!(model.invocation_count(), 1);
}

#[tokio::test]
async fn lifecycle_rejects_operations_before_start_and_after_finish() {
    let runtime = harness(
        Arc::new(FakeModelPort::scripted(vec![Ok(response("unused"))])),
        vec![],
        Arc::new(InMemoryAuditSink::new()),
        AuditFailurePolicy::FailClosed,
    );
    let mut run = context(2, 0, Duration::from_secs(30));
    assert!(matches!(
        runtime.invoke_model(&mut run, request("before")).await,
        Err(HarnessError::InvalidLifecycle { .. })
    ));
    runtime.start_run(&mut run).await.expect("run must start");
    runtime
        .complete_run(&mut run)
        .await
        .expect("run must finish");
    assert!(matches!(
        runtime.invoke_model(&mut run, request("after")).await,
        Err(HarnessError::InvalidLifecycle { .. })
    ));
}

#[tokio::test]
async fn complete_run_keeps_committed_state_when_audit_fails() {
    let audit = Arc::new(FailingAuditSink::on_attempt(2));
    let runtime = harness(
        Arc::new(FakeModelPort::scripted(vec![])),
        vec![],
        audit,
        AuditFailurePolicy::FailClosed,
    );
    let mut run = context(0, 0, Duration::from_secs(30));
    runtime
        .start_run(&mut run)
        .await
        .expect("start audit must succeed");

    let error = runtime
        .complete_run(&mut run)
        .await
        .expect_err("terminal audit is scripted to fail");

    assert!(matches!(
        error,
        HarnessError::Audit {
            phase: AuditPhase::Lifecycle,
            effect: OperationEffect::StateCommitted,
            ..
        }
    ));
    assert_eq!(run.status(), &RunStatus::Finished(RunOutcome::Completed));
}

#[tokio::test]
async fn fail_closed_start_audit_failure_leaves_run_terminal() {
    let runtime = harness(
        Arc::new(FakeModelPort::scripted(vec![])),
        vec![],
        Arc::new(FailingAuditSink::always()),
        AuditFailurePolicy::FailClosed,
    );
    let mut run = context(0, 0, Duration::from_secs(30));

    let error = harness_error(
        runtime.start_run(&mut run).await,
        "start audit must fail closed",
    );

    assert!(matches!(
        error,
        HarnessError::Audit {
            effect: OperationEffect::StateCommitted,
            ..
        }
    ));
    assert_eq!(
        run.status(),
        &RunStatus::Finished(RunOutcome::Failed {
            kind: RunFailureKind::AuditUnavailable,
        })
    );
}

#[tokio::test]
async fn event_sequences_and_model_call_correlation_are_deterministic() {
    let audit = Arc::new(InMemoryAuditSink::new());
    let runtime = harness(
        Arc::new(FakeModelPort::scripted(vec![Ok(response("output"))])),
        vec![],
        audit.clone(),
        AuditFailurePolicy::FailClosed,
    );
    let mut run = context(1, 0, Duration::from_secs(30));
    runtime.start_run(&mut run).await.expect("run must start");
    runtime
        .invoke_model(&mut run, request("prompt"))
        .await
        .expect("model must succeed");
    runtime
        .complete_run(&mut run)
        .await
        .expect("run must finish");

    let events = audit.events();
    assert_eq!(
        events
            .iter()
            .map(|event| event.sequence().get())
            .collect::<Vec<_>>(),
        vec![0, 1, 2, 3]
    );
    let started = match events[1].kind() {
        AgentEventKind::ModelInvocationStarted { model_call_id, .. } => *model_call_id,
        _ => panic!("expected model-started event"),
    };
    let completed = match events[2].kind() {
        AgentEventKind::ModelInvocationCompleted { model_call_id, .. } => *model_call_id,
        _ => panic!("expected model-completed event"),
    };
    assert_eq!(started, completed);
}

#[tokio::test]
async fn fail_closed_pre_audit_prevents_adapter_but_keeps_reservation() {
    let model = Arc::new(FakeModelPort::scripted(vec![Ok(response("unused"))]));
    let runtime = harness(
        model.clone(),
        vec![],
        Arc::new(FailingAuditSink::on_attempt(2)),
        AuditFailurePolicy::FailClosed,
    );
    let mut run = context(1, 0, Duration::from_secs(30));
    runtime.start_run(&mut run).await.expect("run must start");

    let error = harness_error(
        runtime.invoke_model(&mut run, request("prompt")).await,
        "pre-audit must fail closed",
    );

    assert!(matches!(
        error,
        HarnessError::Audit {
            phase: AuditPhase::BeforeInvocation,
            effect: OperationEffect::NotInvoked,
            ..
        }
    ));
    assert_eq!(run.usage().model_calls(), 1);
    assert_eq!(model.invocation_count(), 0);
    assert_eq!(run.status(), &RunStatus::Running);

    let exhausted = harness_error(
        runtime
            .invoke_model(&mut run, request("next attempt"))
            .await,
        "the next reservation must encounter the consumed limit",
    );
    assert_eq!(
        exhausted.budget_dimension(),
        Some(BudgetDimension::ModelCalls)
    );
    assert_eq!(
        run.status(),
        &RunStatus::Finished(RunOutcome::BudgetExceeded {
            dimension: BudgetDimension::ModelCalls,
        })
    );
}

#[tokio::test]
async fn fail_open_pre_audit_invokes_adapter_and_marks_audit_degraded() {
    let model = Arc::new(FakeModelPort::scripted(vec![Ok(response("output"))]));
    let audit = Arc::new(FailingAuditSink::on_attempt(2));
    let runtime = harness(
        model.clone(),
        vec![],
        audit.clone(),
        AuditFailurePolicy::FailOpen,
    );
    let mut run = context(1, 0, Duration::from_secs(30));
    runtime.start_run(&mut run).await.expect("run must start");

    runtime
        .invoke_model(&mut run, request("prompt"))
        .await
        .expect("fail-open must preserve operation result");

    assert!(run.audit_degraded());
    assert_eq!(model.invocation_count(), 1);
    assert_eq!(
        audit
            .recorded_events()
            .iter()
            .map(|event| event.sequence().get())
            .collect::<Vec<_>>(),
        vec![0, 2]
    );
}

#[tokio::test]
async fn post_invocation_audit_failure_reports_invocation_started() {
    let model = Arc::new(FakeModelPort::scripted(vec![Ok(response("output"))]));
    let runtime = harness(
        model.clone(),
        vec![],
        Arc::new(FailingAuditSink::on_attempt(3)),
        AuditFailurePolicy::FailClosed,
    );
    let mut run = context(1, 0, Duration::from_secs(30));
    runtime.start_run(&mut run).await.expect("run must start");

    let error = harness_error(
        runtime.invoke_model(&mut run, request("prompt")).await,
        "post-audit must fail closed",
    );

    assert!(matches!(
        error,
        HarnessError::Audit {
            phase: AuditPhase::AfterInvocation,
            effect: OperationEffect::InvocationStarted,
            ..
        }
    ));
    assert_eq!(model.invocation_count(), 1);
}

#[tokio::test]
async fn fail_open_post_audit_preserves_adapter_error() {
    let model = Arc::new(FakeModelPort::scripted(vec![Err(ModelPortError::Rejected)]));
    let runtime = harness(
        model.clone(),
        vec![],
        Arc::new(FailingAuditSink::on_attempt(3)),
        AuditFailurePolicy::FailOpen,
    );
    let mut run = context(1, 0, Duration::from_secs(30));
    runtime.start_run(&mut run).await.expect("run must start");

    let error = harness_error(
        runtime.invoke_model(&mut run, request("prompt")).await,
        "adapter failure must remain an error",
    );

    assert_eq!(error, HarnessError::ModelPort(ModelPortError::Rejected));
    assert!(run.audit_degraded());
    assert_eq!(model.invocation_count(), 1);
}

#[tokio::test]
async fn terminal_safety_state_wins_when_terminal_audit_fails() {
    let model = Arc::new(FakeModelPort::scripted(vec![Ok(response("unused"))]));
    let runtime = harness(
        model.clone(),
        vec![],
        Arc::new(FailingAuditSink::on_attempt(2)),
        AuditFailurePolicy::FailClosed,
    );
    let mut run = context(1, 0, Duration::from_secs(30));
    let cancellation = runtime.start_run(&mut run).await.expect("run must start");
    cancellation.request_cancel();

    let error = harness_error(
        runtime.invoke_model(&mut run, request("prompt")).await,
        "cancellation must win",
    );

    assert!(matches!(error, HarnessError::Cancelled { .. }));
    assert_eq!(run.status(), &RunStatus::Finished(RunOutcome::Cancelled));
    assert!(run.audit_degraded());
    assert_eq!(model.invocation_count(), 0);
}

#[tokio::test]
async fn terminal_audit_failure_preserves_call_budget_exhaustion_states() {
    let model = Arc::new(FakeModelPort::scripted(vec![Ok(response("unused"))]));
    let model_runtime = harness(
        model.clone(),
        vec![],
        Arc::new(FailingAuditSink::on_attempt(2)),
        AuditFailurePolicy::FailClosed,
    );
    let mut model_run = context(0, 0, Duration::from_secs(30));
    model_runtime
        .start_run(&mut model_run)
        .await
        .expect("run must start");

    let model_error = harness_error(
        model_runtime
            .invoke_model(&mut model_run, request("unused"))
            .await,
        "zero model budget must be exhausted",
    );
    assert_eq!(
        model_error.budget_dimension(),
        Some(BudgetDimension::ModelCalls)
    );
    assert_eq!(
        model_run.status(),
        &RunStatus::Finished(RunOutcome::BudgetExceeded {
            dimension: BudgetDimension::ModelCalls,
        })
    );
    assert!(model_run.audit_degraded());
    assert_eq!(model.invocation_count(), 0);

    let tool = Arc::new(FakeToolPort::scripted(
        definition("lookup", CapabilityKind::ReadOnly),
        vec![],
    ));
    let tool_runtime = harness(
        Arc::new(FakeModelPort::scripted(vec![])),
        vec![tool.clone()],
        Arc::new(FailingAuditSink::on_attempt(2)),
        AuditFailurePolicy::FailClosed,
    );
    let mut tool_run = context(0, 0, Duration::from_secs(30));
    tool_runtime
        .start_run(&mut tool_run)
        .await
        .expect("run must start");

    let tool_error = tool_runtime
        .invoke_tool(&mut tool_run, call("lookup", ToolCallId::new(), "unused"))
        .await
        .expect_err("zero tool budget must be exhausted");
    assert_eq!(
        tool_error.budget_dimension(),
        Some(BudgetDimension::ToolCalls)
    );
    assert_eq!(
        tool_run.status(),
        &RunStatus::Finished(RunOutcome::BudgetExceeded {
            dimension: BudgetDimension::ToolCalls,
        })
    );
    assert!(tool_run.audit_degraded());
    assert_eq!(tool.invocation_count(), 0);
}

#[tokio::test]
async fn adapter_errors_are_sanitized_and_domain_failure_is_a_normal_result() {
    let call_id = ToolCallId::new();
    let domain_tool = Arc::new(FakeToolPort::scripted(
        definition("domain", CapabilityKind::ReadOnly),
        vec![Ok(ToolResult::DomainFailure {
            call_id,
            failure: ToolDomainFailure::new(ToolDomainFailureKind::Rejected),
        })],
    ));
    let adapter_tool = Arc::new(FakeToolPort::scripted(
        definition("adapter", CapabilityKind::ReadOnly),
        vec![Err(ToolPortError::AdapterFailure)],
    ));
    let runtime = harness(
        Arc::new(FakeModelPort::scripted(vec![Err(ModelPortError::Failed)])),
        vec![domain_tool, adapter_tool],
        Arc::new(InMemoryAuditSink::new()),
        AuditFailurePolicy::FailClosed,
    );
    let mut run = context(1, 2, Duration::from_secs(30));
    runtime.start_run(&mut run).await.expect("run must start");

    let model_error = harness_error(
        runtime
            .invoke_model(&mut run, request("provider-secret-sentinel"))
            .await,
        "model adapter is scripted to fail",
    );
    assert_eq!(model_error, HarnessError::ModelPort(ModelPortError::Failed));
    assert!(!model_error.to_string().contains("provider-secret-sentinel"));

    let domain = runtime
        .invoke_tool(&mut run, call("domain", call_id, "domain-input"))
        .await
        .expect("domain failure is a successful port invocation");
    assert!(matches!(domain, ToolResult::DomainFailure { .. }));

    let tool_error = runtime
        .invoke_tool(
            &mut run,
            call("adapter", ToolCallId::new(), "adapter-input"),
        )
        .await
        .expect_err("tool adapter is scripted to fail");
    assert_eq!(
        tool_error,
        HarnessError::ToolPort(ToolPortError::AdapterFailure)
    );
}

#[tokio::test]
async fn serialized_events_never_contain_raw_payload_or_error_sentinels() {
    const PROMPT: &str = "sentinel-raw-prompt";
    const MODEL_OUTPUT: &str = "sentinel-raw-model-output";
    const TOOL_INPUT: &str = "sentinel-raw-tool-input";
    const TOOL_OUTPUT: &str = "sentinel-raw-tool-output";
    const PROVIDER_ERROR: &str = "sentinel-provider-error";

    let call_id = ToolCallId::new();
    let tool = Arc::new(FakeToolPort::scripted(
        definition("lookup", CapabilityKind::ReadOnly),
        vec![Ok(ToolResult::Succeeded {
            call_id,
            output: ToolOutput::new(serde_json::json!({"value": TOOL_OUTPUT})),
        })],
    ));
    let audit = Arc::new(InMemoryAuditSink::new());
    let runtime = harness(
        Arc::new(FakeModelPort::scripted(vec![Ok(response(MODEL_OUTPUT))])),
        vec![tool],
        audit.clone(),
        AuditFailurePolicy::FailClosed,
    );
    let mut run = context(1, 1, Duration::from_secs(30));
    runtime.start_run(&mut run).await.expect("run must start");
    runtime
        .invoke_model(&mut run, request(PROMPT))
        .await
        .expect("model must succeed");
    runtime
        .invoke_tool(&mut run, call("lookup", call_id, TOOL_INPUT))
        .await
        .expect("tool must succeed");
    runtime
        .complete_run(&mut run)
        .await
        .expect("run must finish");

    let serialized = serde_json::to_string(&audit.events()).expect("events must serialize");
    for sentinel in [
        PROMPT,
        MODEL_OUTPUT,
        TOOL_INPUT,
        TOOL_OUTPUT,
        PROVIDER_ERROR,
    ] {
        assert!(!serialized.contains(sentinel));
    }
}

#[tokio::test]
async fn begin_iteration_reserves_one_based_usage_and_audits_metadata() {
    let audit = Arc::new(InMemoryAuditSink::new());
    let runtime = harness(
        Arc::new(FakeModelPort::scripted(vec![])),
        vec![],
        audit.clone(),
        AuditFailurePolicy::FailClosed,
    );
    let mut run = context_with_iterations(0, 0, 2, Duration::from_secs(30));
    runtime.start_run(&mut run).await.expect("run must start");

    let iteration = runtime
        .begin_iteration(&mut run)
        .await
        .expect("iteration reservation must succeed");

    assert_eq!(iteration, 1);
    assert_eq!(run.usage().iterations(), 1);
    assert!(matches!(
        audit.events()[1].kind(),
        AgentEventKind::Loop {
            event: LoopEventKind::IterationStarted {
                iteration: 1,
                usage: 1,
                limit: 2,
            }
        }
    ));
}

#[tokio::test]
async fn zero_iteration_budget_terminalizes_before_any_iteration_event() {
    let audit = Arc::new(InMemoryAuditSink::new());
    let runtime = harness(
        Arc::new(FakeModelPort::scripted(vec![])),
        vec![],
        audit.clone(),
        AuditFailurePolicy::FailClosed,
    );
    let mut run = context_with_iterations(0, 0, 0, Duration::from_secs(30));
    runtime.start_run(&mut run).await.expect("run must start");

    let error = runtime
        .begin_iteration(&mut run)
        .await
        .expect_err("zero iteration budget must reject the reservation");

    assert_eq!(error.budget_dimension(), Some(BudgetDimension::Iterations));
    assert_eq!(run.usage().iterations(), 0);
    assert_eq!(
        run.status(),
        &RunStatus::Finished(RunOutcome::BudgetExceeded {
            dimension: BudgetDimension::Iterations,
        })
    );
    assert!(audit.events().iter().all(|event| !matches!(
        event.kind(),
        AgentEventKind::Loop {
            event: LoopEventKind::IterationStarted { .. }
        }
    )));
}

#[tokio::test]
async fn exhausted_iteration_budget_preserves_started_iteration_usage() {
    let runtime = harness(
        Arc::new(FakeModelPort::scripted(vec![])),
        vec![],
        Arc::new(InMemoryAuditSink::new()),
        AuditFailurePolicy::FailClosed,
    );
    let mut run = context_with_iterations(0, 0, 1, Duration::from_secs(30));
    runtime.start_run(&mut run).await.expect("run must start");
    assert_eq!(runtime.begin_iteration(&mut run).await, Ok(1));

    let error = runtime
        .begin_iteration(&mut run)
        .await
        .expect_err("second iteration must exceed the configured limit");

    assert_eq!(error.budget_dimension(), Some(BudgetDimension::Iterations));
    assert_eq!(run.usage().iterations(), 1);
    assert_eq!(
        run.status(),
        &RunStatus::Finished(RunOutcome::BudgetExceeded {
            dimension: BudgetDimension::Iterations,
        })
    );
}

#[tokio::test]
async fn fail_closed_iteration_started_audit_keeps_reservation_non_terminal() {
    let runtime = harness(
        Arc::new(FakeModelPort::scripted(vec![])),
        vec![],
        Arc::new(FailingAuditSink::on_attempt(2)),
        AuditFailurePolicy::FailClosed,
    );
    let mut run = context_with_iterations(0, 0, 1, Duration::from_secs(30));
    runtime.start_run(&mut run).await.expect("run must start");

    let error = runtime
        .begin_iteration(&mut run)
        .await
        .expect_err("iteration-start audit must fail closed");

    assert!(matches!(
        error,
        HarnessError::Audit {
            operation: crate::HarnessOperation::BeginIteration,
            phase: AuditPhase::Lifecycle,
            effect: OperationEffect::StateCommitted,
            ..
        }
    ));
    assert_eq!(run.usage().iterations(), 1);
    assert_eq!(run.status(), &RunStatus::Running);
}

#[tokio::test]
async fn fail_open_iteration_started_audit_marks_degraded_and_continues() {
    let runtime = harness(
        Arc::new(FakeModelPort::scripted(vec![])),
        vec![],
        Arc::new(FailingAuditSink::on_attempt(2)),
        AuditFailurePolicy::FailOpen,
    );
    let mut run = context_with_iterations(0, 0, 1, Duration::from_secs(30));
    runtime.start_run(&mut run).await.expect("run must start");

    assert_eq!(runtime.begin_iteration(&mut run).await, Ok(1));
    assert_eq!(run.usage().iterations(), 1);
    assert!(run.audit_degraded());
    assert_eq!(run.status(), &RunStatus::Running);
}

#[tokio::test]
async fn loop_progress_is_metadata_only_and_sequence_ordered() {
    let audit = Arc::new(InMemoryAuditSink::new());
    let runtime = harness(
        Arc::new(FakeModelPort::scripted(vec![])),
        vec![],
        audit.clone(),
        AuditFailurePolicy::FailClosed,
    );
    let mut run = context_with_iterations(0, 0, 1, Duration::from_secs(30));
    runtime.start_run(&mut run).await.expect("run must start");
    runtime
        .begin_iteration(&mut run)
        .await
        .expect("iteration must begin");
    runtime
        .record_loop_progress(
            &mut run,
            LoopProgressEvent::PhaseEntered {
                iteration: 1,
                phase: LoopPhase::Observe,
            },
        )
        .await
        .expect("progress must be recorded");

    let events = audit.events();
    assert_eq!(
        events
            .iter()
            .map(|event| event.sequence().get())
            .collect::<Vec<_>>(),
        vec![0, 1, 2]
    );
    let serialized = serde_json::to_string(&events).expect("events must serialize");
    for sentinel in ["raw prompt", "model output", "tool input", "provider error"] {
        assert!(!serialized.contains(sentinel));
    }
}

#[tokio::test]
async fn checkpoint_terminalizes_requested_cancellation_without_consuming_budget() {
    let runtime = harness(
        Arc::new(FakeModelPort::scripted(vec![])),
        vec![],
        Arc::new(InMemoryAuditSink::new()),
        AuditFailurePolicy::FailClosed,
    );
    let mut run = context_with_iterations(1, 1, 1, Duration::from_secs(30));
    let cancellation = runtime.start_run(&mut run).await.expect("run must start");
    cancellation.request_cancel();

    let error = runtime
        .checkpoint(&mut run)
        .await
        .expect_err("checkpoint must observe cancellation");

    assert!(matches!(error, HarnessError::Cancelled { .. }));
    assert_eq!(run.status(), &RunStatus::Finished(RunOutcome::Cancelled));
    assert_eq!(run.usage().model_calls(), 0);
    assert_eq!(run.usage().tool_calls(), 0);
    assert_eq!(run.usage().iterations(), 0);
}

#[tokio::test(start_paused = true)]
async fn checkpoint_terminalizes_elapsed_deadline_without_consuming_budget() {
    let runtime = harness(
        Arc::new(FakeModelPort::scripted(vec![])),
        vec![],
        Arc::new(InMemoryAuditSink::new()),
        AuditFailurePolicy::FailClosed,
    );
    let mut run = context_with_iterations(1, 1, 1, Duration::from_secs(1));
    runtime.start_run(&mut run).await.expect("run must start");
    tokio::time::advance(Duration::from_secs(1)).await;

    let error = runtime
        .checkpoint(&mut run)
        .await
        .expect_err("checkpoint must observe elapsed deadline");

    assert!(matches!(error, HarnessError::DeadlineExceeded { .. }));
    assert_eq!(
        run.status(),
        &RunStatus::Finished(RunOutcome::BudgetExceeded {
            dimension: BudgetDimension::Elapsed,
        })
    );
    assert_eq!(run.usage().model_calls(), 0);
    assert_eq!(run.usage().tool_calls(), 0);
    assert_eq!(run.usage().iterations(), 0);
}
