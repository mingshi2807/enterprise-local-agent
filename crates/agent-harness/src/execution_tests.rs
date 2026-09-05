use std::{sync::Arc, time::Duration};

use agent_core::{
    AgentEventKind, BudgetDimension, CapabilityKind, LoopEventKind, LoopPhase, LoopProgressEvent,
    ModelMessage, ModelOutputPart, ModelRequest, ModelResponse, ModelRole, RunBudget,
    RunFailureKind, RunId, RunOutcome, RunStatus, SessionId, ToolCall, ToolCallId, ToolDefinition,
    ToolDomainFailure, ToolDomainFailureKind, ToolInput, ToolName, ToolOutput, ToolResult,
    ToolSchema,
};

use crate::{
    AppendTransition, ApprovalDecision, ApprovalPort, ApprovalPortError, AuditFailurePolicy,
    AuditPhase, AuditSink, AuthorizationDecision, CapabilityPolicy, ContainedToolPort,
    ContainmentPortError, DurableCheckpoint, ExecutionHarness, ExecutionStage, HarnessConfig,
    HarnessError, LoadedRun, M0ReadOnlyPolicy, M6ApprovalPolicy, ModelPort, ModelPortError,
    OperationEffect, PersistenceFuture, PersistencePortError, PolicyDenial, PolicyDenialReason,
    PortFuture, RunContext, RunPersistencePort, RunRecord, ToolPort, ToolPortError, ToolRegistry,
    testing::{
        FailingAuditSink, FakeContainedToolPort, FakeInvocation, FakeModelPort, FakeToolPort,
        InMemoryAuditSink, InvocationLog, PendingApprovalPort, ScriptedApprovalPort,
    },
};

struct RejectEffectStartPersistence;

impl RunPersistencePort for RejectEffectStartPersistence {
    fn create_run<'a>(
        &'a self,
        _record: &'a RunRecord,
        _initial_checkpoint: &'a DurableCheckpoint,
    ) -> PersistenceFuture<'a, Result<(), PersistencePortError>> {
        Box::pin(std::future::ready(Ok(())))
    }

    fn append_transition<'a>(
        &'a self,
        transition: &'a AppendTransition,
    ) -> PersistenceFuture<'a, Result<(), PersistencePortError>> {
        Box::pin(std::future::ready(
            if transition.event().sequence().get() == 0 {
                Ok(())
            } else {
                Err(PersistencePortError::Unavailable)
            },
        ))
    }

    fn load_run<'a>(
        &'a self,
        _key: crate::RunKey,
    ) -> PersistenceFuture<'a, Result<LoadedRun, PersistencePortError>> {
        Box::pin(std::future::ready(Err(PersistencePortError::Unavailable)))
    }
}

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

fn context_with_approval_budget(
    model_calls: u32,
    tool_calls: u32,
    approval_requests: u32,
    elapsed: Duration,
) -> RunContext {
    RunContext::new(
        RunId::new(),
        SessionId::new(),
        RunBudget::new(model_calls, tool_calls, 1, elapsed)
            .expect("test budget must be valid")
            .with_max_approval_requests(approval_requests),
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
async fn durable_effect_start_failure_invokes_no_external_port() {
    let model = Arc::new(FakeModelPort::scripted(vec![Ok(response("unused"))]));
    let runtime = harness(
        model.clone(),
        Vec::new(),
        Arc::new(InMemoryAuditSink::new()),
        AuditFailurePolicy::FailClosed,
    )
    .with_persistence_port(Arc::new(RejectEffectStartPersistence));
    let mut run = context(1, 0, Duration::from_secs(30));
    runtime.start_run(&mut run).await.expect("run must start");

    assert!(matches!(
        runtime
            .invoke_model(&mut run, request("never dispatched"))
            .await,
        Err(HarnessError::Persistence(PersistencePortError::Unavailable))
    ));
    assert_eq!(model.invocation_count(), 0);
}

fn harness_with_registry(
    model: Arc<FakeModelPort>,
    registry: ToolRegistry,
    audit: Arc<dyn AuditSink>,
    policy: AuditFailurePolicy,
    capability_policy: Arc<dyn CapabilityPolicy>,
    approval: Option<Arc<dyn ApprovalPort>>,
) -> ExecutionHarness {
    let model: Arc<dyn ModelPort> = model;
    let harness = ExecutionHarness::new(model, registry, capability_policy, audit, config(policy));
    match approval {
        Some(approval) => harness.with_approval_port(approval),
        None => harness,
    }
}

async fn prepare_single_action(
    runtime: &ExecutionHarness,
    run: &mut RunContext,
) -> crate::ValidatedAction {
    let invocation = runtime
        .invoke_model_tracked(run, request("plan"))
        .await
        .expect("model invocation must succeed");
    runtime
        .prepare_action(run, invocation)
        .await
        .expect("action must prepare")
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
async fn contained_read_only_executes_without_approval_or_approval_budget() {
    let contained = Arc::new(FakeContainedToolPort::succeeding(
        definition("lookup", CapabilityKind::ReadOnly),
        ToolOutput::new(serde_json::json!({"ok": true})),
    ));
    let contained_port: Arc<dyn ContainedToolPort> = contained.clone();
    let mut registry = ToolRegistry::new();
    registry
        .register_contained(contained_port)
        .expect("contained read-only must register");
    let runtime = harness_with_registry(
        Arc::new(FakeModelPort::scripted(vec![])),
        registry,
        Arc::new(InMemoryAuditSink::new()),
        AuditFailurePolicy::FailClosed,
        Arc::new(M6ApprovalPolicy),
        None,
    );
    let mut run = context_with_approval_budget(0, 1, 0, Duration::from_secs(30));
    runtime.start_run(&mut run).await.expect("run must start");

    runtime
        .invoke_tool(&mut run, call("lookup", ToolCallId::new(), "input"))
        .await
        .expect("contained read-only must execute without approval");

    assert_eq!(contained.invocation_count(), 1);
    assert_eq!(contained.preview_count(), 0);
    assert_eq!(run.usage().tool_calls(), 1);
    assert_eq!(run.usage().approval_requests(), 0);
}

#[tokio::test]
async fn direct_local_write_cannot_request_approval_or_execute() {
    let model = Arc::new(FakeModelPort::scripted(vec![Ok(response(
        r#"{"action":{"tool":"write_note","arguments":{}}}"#,
    ))]));
    let approval = Arc::new(ScriptedApprovalPort::approve_all());
    let tool = Arc::new(FakeToolPort::succeeding(
        definition("write_note", CapabilityKind::LocalWrite),
        ToolOutput::new(serde_json::json!({})),
    ));
    let mut registry = ToolRegistry::new();
    let tool_port: Arc<dyn ToolPort> = tool.clone();
    registry.register(tool_port).expect("tool must register");
    let runtime = harness_with_registry(
        model,
        registry,
        Arc::new(InMemoryAuditSink::new()),
        AuditFailurePolicy::FailClosed,
        Arc::new(M6ApprovalPolicy),
        Some(approval.clone()),
    );
    let mut run = context_with_approval_budget(1, 1, 1, Duration::from_secs(30));
    runtime.start_run(&mut run).await.expect("run must start");
    let action = prepare_single_action(&runtime, &mut run).await;

    let error = runtime
        .invoke_validated_action(&mut run, action)
        .await
        .expect_err("direct local write must fail before approval");

    assert!(matches!(error, HarnessError::ContainmentUnavailable { .. }));
    assert_eq!(approval.invocation_count(), 0);
    assert_eq!(tool.invocation_count(), 0);
    assert_eq!(run.usage().approval_requests(), 0);
    assert_eq!(run.usage().tool_calls(), 0);
}

#[tokio::test]
async fn contained_local_write_approval_grant_executes_once() {
    let model = Arc::new(FakeModelPort::scripted(vec![Ok(response(
        r#"{"action":{"tool":"write_note","arguments":{}}}"#,
    ))]));
    let approval = Arc::new(ScriptedApprovalPort::approve_all());
    let contained = Arc::new(FakeContainedToolPort::succeeding(
        definition("write_note", CapabilityKind::LocalWrite),
        ToolOutput::new(serde_json::json!({"ok": true})),
    ));
    let contained_port: Arc<dyn ContainedToolPort> = contained.clone();
    let mut registry = ToolRegistry::new();
    registry
        .register_contained(contained_port)
        .expect("contained local write must register");
    let audit = Arc::new(InMemoryAuditSink::new());
    let runtime = harness_with_registry(
        model,
        registry,
        audit.clone(),
        AuditFailurePolicy::FailClosed,
        Arc::new(M6ApprovalPolicy),
        Some(approval.clone()),
    );
    let mut run = context_with_approval_budget(1, 1, 1, Duration::from_secs(30));
    runtime.start_run(&mut run).await.expect("run must start");
    let action = prepare_single_action(&runtime, &mut run).await;

    runtime
        .invoke_validated_action(&mut run, action)
        .await
        .expect("approved contained local write must execute");

    assert_eq!(approval.invocation_count(), 1);
    assert_eq!(contained.preview_count(), 1);
    assert_eq!(contained.invocation_count(), 1);
    assert_eq!(contained.termination_count(), 0);
    assert_eq!(run.usage().approval_requests(), 1);
    assert_eq!(run.usage().tool_calls(), 1);
    let event_kinds: Vec<_> = audit
        .events()
        .into_iter()
        .filter_map(|event| match event.kind() {
            AgentEventKind::ApprovalRequested { .. } => Some("approval_requested"),
            AgentEventKind::ApprovalGranted { .. } => Some("approval_granted"),
            AgentEventKind::ToolInvocationStarted { .. } => Some("tool_started"),
            AgentEventKind::ToolInvocationCompleted { .. } => Some("tool_completed"),
            _ => None,
        })
        .collect();
    assert_eq!(
        event_kinds,
        vec![
            "approval_requested",
            "approval_granted",
            "tool_started",
            "tool_completed"
        ]
    );
}

#[tokio::test]
async fn approval_denial_consumes_approval_budget_but_zero_tool_calls() {
    let model = Arc::new(FakeModelPort::scripted(vec![Ok(response(
        r#"{"action":{"tool":"write_note","arguments":{}}}"#,
    ))]));
    let approval = Arc::new(ScriptedApprovalPort::deny_all());
    let contained = Arc::new(FakeContainedToolPort::succeeding(
        definition("write_note", CapabilityKind::LocalWrite),
        ToolOutput::new(serde_json::json!({})),
    ));
    let contained_port: Arc<dyn ContainedToolPort> = contained.clone();
    let mut registry = ToolRegistry::new();
    registry
        .register_contained(contained_port)
        .expect("contained local write must register");
    let runtime = harness_with_registry(
        model,
        registry,
        Arc::new(InMemoryAuditSink::new()),
        AuditFailurePolicy::FailClosed,
        Arc::new(M6ApprovalPolicy),
        Some(approval.clone()),
    );
    let mut run = context_with_approval_budget(1, 1, 1, Duration::from_secs(30));
    runtime.start_run(&mut run).await.expect("run must start");
    let action = prepare_single_action(&runtime, &mut run).await;

    let error = runtime
        .invoke_validated_action(&mut run, action)
        .await
        .expect_err("approval denial must fail closed");

    assert!(matches!(error, HarnessError::ApprovalDenied));
    assert_eq!(approval.invocation_count(), 1);
    assert_eq!(contained.invocation_count(), 0);
    assert_eq!(run.usage().approval_requests(), 1);
    assert_eq!(run.usage().tool_calls(), 0);
}

#[tokio::test]
async fn zero_approval_budget_terminalizes_before_approval_port() {
    let model = Arc::new(FakeModelPort::scripted(vec![Ok(response(
        r#"{"action":{"tool":"write_note","arguments":{}}}"#,
    ))]));
    let approval = Arc::new(ScriptedApprovalPort::approve_all());
    let contained = Arc::new(FakeContainedToolPort::succeeding(
        definition("write_note", CapabilityKind::LocalWrite),
        ToolOutput::new(serde_json::json!({})),
    ));
    let contained_port: Arc<dyn ContainedToolPort> = contained.clone();
    let mut registry = ToolRegistry::new();
    registry
        .register_contained(contained_port)
        .expect("contained local write must register");
    let runtime = harness_with_registry(
        model,
        registry,
        Arc::new(InMemoryAuditSink::new()),
        AuditFailurePolicy::FailClosed,
        Arc::new(M6ApprovalPolicy),
        Some(approval.clone()),
    );
    let mut run = context_with_approval_budget(1, 1, 0, Duration::from_secs(30));
    runtime.start_run(&mut run).await.expect("run must start");
    let action = prepare_single_action(&runtime, &mut run).await;

    let error = runtime
        .invoke_validated_action(&mut run, action)
        .await
        .expect_err("approval budget must be hard");

    assert_eq!(
        error.budget_dimension(),
        Some(BudgetDimension::ApprovalRequests)
    );
    assert_eq!(approval.invocation_count(), 0);
    assert_eq!(contained.invocation_count(), 0);
    assert_eq!(
        run.status(),
        &RunStatus::Finished(RunOutcome::BudgetExceeded {
            dimension: BudgetDimension::ApprovalRequests,
        })
    );
}

#[derive(Clone, Copy)]
struct AlwaysAllowPolicy;

impl CapabilityPolicy for AlwaysAllowPolicy {
    fn authorize(&self, _capability: CapabilityKind) -> AuthorizationDecision {
        AuthorizationDecision::Allowed
    }
}

#[derive(Clone, Copy)]
struct AlwaysDenyPolicy;

impl CapabilityPolicy for AlwaysDenyPolicy {
    fn authorize(&self, capability: CapabilityKind) -> AuthorizationDecision {
        AuthorizationDecision::Denied(PolicyDenial::new(
            capability,
            PolicyDenialReason::CapabilityNotExecutableInM6,
        ))
    }
}

#[tokio::test]
async fn validated_read_only_action_still_obeys_capability_policy() {
    let model = Arc::new(FakeModelPort::scripted(vec![Ok(response(
        r#"{"action":{"tool":"lookup","arguments":{}}}"#,
    ))]));
    let tool = Arc::new(FakeToolPort::succeeding(
        definition("lookup", CapabilityKind::ReadOnly),
        ToolOutput::new(serde_json::json!({})),
    ));
    let mut registry = ToolRegistry::new();
    let tool_port: Arc<dyn ToolPort> = tool.clone();
    registry.register(tool_port).expect("tool must register");
    let runtime = harness_with_registry(
        model,
        registry,
        Arc::new(InMemoryAuditSink::new()),
        AuditFailurePolicy::FailClosed,
        Arc::new(AlwaysDenyPolicy),
        None,
    );
    let mut run = context_with_approval_budget(1, 1, 0, Duration::from_secs(30));
    runtime.start_run(&mut run).await.expect("run must start");
    let action = prepare_single_action(&runtime, &mut run).await;

    let error = runtime
        .invoke_validated_action(&mut run, action)
        .await
        .expect_err("policy denial must remain authoritative for read-only actions");

    assert!(matches!(error, HarnessError::PolicyDenied(_)));
    assert_eq!(tool.invocation_count(), 0);
    assert_eq!(run.usage().tool_calls(), 0);
    assert_eq!(run.usage().approval_requests(), 0);
}

#[tokio::test]
async fn faulty_allowed_local_write_cannot_bypass_approval_requirement() {
    let model = Arc::new(FakeModelPort::scripted(vec![Ok(response(
        r#"{"action":{"tool":"write_note","arguments":{}}}"#,
    ))]));
    let approval = Arc::new(ScriptedApprovalPort::approve_all());
    let contained = Arc::new(FakeContainedToolPort::succeeding(
        definition("write_note", CapabilityKind::LocalWrite),
        ToolOutput::new(serde_json::json!({})),
    ));
    let contained_port: Arc<dyn ContainedToolPort> = contained.clone();
    let mut registry = ToolRegistry::new();
    registry
        .register_contained(contained_port)
        .expect("contained local write must register");
    let runtime = harness_with_registry(
        model,
        registry,
        Arc::new(InMemoryAuditSink::new()),
        AuditFailurePolicy::FailClosed,
        Arc::new(AlwaysAllowPolicy),
        Some(approval.clone()),
    );
    let mut run = context_with_approval_budget(1, 1, 1, Duration::from_secs(30));
    runtime.start_run(&mut run).await.expect("run must start");
    let action = prepare_single_action(&runtime, &mut run).await;

    let error = runtime
        .invoke_validated_action(&mut run, action)
        .await
        .expect_err("faulty allowed local write must still fail");

    assert!(matches!(error, HarnessError::ApprovalRequired));
    assert_eq!(approval.invocation_count(), 0);
    assert_eq!(contained.invocation_count(), 0);
    assert_eq!(run.usage().approval_requests(), 0);
    assert_eq!(run.usage().tool_calls(), 0);
}

#[tokio::test]
async fn faulty_allowed_external_write_and_privileged_cannot_execute() {
    for capability in [CapabilityKind::ExternalWrite, CapabilityKind::Privileged] {
        let model = Arc::new(FakeModelPort::scripted(vec![Ok(response(
            r#"{"action":{"tool":"danger","arguments":{}}}"#,
        ))]));
        let tool = Arc::new(FakeToolPort::succeeding(
            definition("danger", capability),
            ToolOutput::new(serde_json::json!({})),
        ));
        let tool_port: Arc<dyn ToolPort> = tool.clone();
        let mut registry = ToolRegistry::new();
        registry.register(tool_port).expect("tool must register");
        let runtime = harness_with_registry(
            model,
            registry,
            Arc::new(InMemoryAuditSink::new()),
            AuditFailurePolicy::FailClosed,
            Arc::new(AlwaysAllowPolicy),
            None,
        );
        let mut run = context_with_approval_budget(1, 1, 1, Duration::from_secs(30));
        runtime.start_run(&mut run).await.expect("run must start");
        let action = prepare_single_action(&runtime, &mut run).await;

        let error = runtime
            .invoke_validated_action(&mut run, action)
            .await
            .expect_err("unsupported capability must never execute");

        assert!(matches!(
            error,
            HarnessError::CapabilityNotExecutable { capability: denied } if denied == capability
        ));
        assert_eq!(tool.invocation_count(), 0);
        assert_eq!(run.usage().approval_requests(), 0);
        assert_eq!(run.usage().tool_calls(), 0);
    }
}

fn local_write_runtime_with_audit(
    audit: Arc<dyn AuditSink>,
    audit_policy: AuditFailurePolicy,
    approval: Arc<dyn ApprovalPort>,
) -> (ExecutionHarness, Arc<FakeContainedToolPort>) {
    let model = Arc::new(FakeModelPort::scripted(vec![Ok(response(
        r#"{"action":{"tool":"write_note","arguments":{}}}"#,
    ))]));
    let contained = Arc::new(FakeContainedToolPort::succeeding(
        definition("write_note", CapabilityKind::LocalWrite),
        ToolOutput::new(serde_json::json!({})),
    ));
    let contained_port: Arc<dyn ContainedToolPort> = contained.clone();
    let mut registry = ToolRegistry::new();
    registry
        .register_contained(contained_port)
        .expect("contained local write must register");
    (
        harness_with_registry(
            model,
            registry,
            audit,
            audit_policy,
            Arc::new(M6ApprovalPolicy),
            Some(approval),
        ),
        contained,
    )
}

fn local_write_runtime_with_contained(
    contained: Arc<FakeContainedToolPort>,
    audit: Arc<dyn AuditSink>,
    approval: Option<Arc<dyn ApprovalPort>>,
) -> ExecutionHarness {
    let model = Arc::new(FakeModelPort::scripted(vec![Ok(response(
        r#"{"action":{"tool":"write_note","arguments":{}}}"#,
    ))]));
    let contained_port: Arc<dyn ContainedToolPort> = contained;
    let mut registry = ToolRegistry::new();
    registry
        .register_contained(contained_port)
        .expect("contained local write must register");
    harness_with_registry(
        model,
        registry,
        audit,
        AuditFailurePolicy::FailClosed,
        Arc::new(M6ApprovalPolicy),
        approval,
    )
}

#[tokio::test]
async fn degraded_audit_from_action_preparation_blocks_local_write() {
    let approval = Arc::new(ScriptedApprovalPort::approve_all());
    let audit = Arc::new(FailingAuditSink::on_attempt(4));
    let (runtime, contained) =
        local_write_runtime_with_audit(audit, AuditFailurePolicy::FailOpen, approval.clone());
    let mut run = context_with_approval_budget(1, 1, 1, Duration::from_secs(30));
    runtime.start_run(&mut run).await.expect("run must start");
    let action = prepare_single_action(&runtime, &mut run).await;
    assert!(run.audit_degraded());

    let error = runtime
        .invoke_validated_action(&mut run, action)
        .await
        .expect_err("an already-degraded audit state must block local write");

    assert!(matches!(error, HarnessError::AuditDegraded));
    assert_eq!(approval.invocation_count(), 0);
    assert_eq!(contained.preview_count(), 0);
    assert_eq!(contained.invocation_count(), 0);
    assert_eq!(run.usage().approval_requests(), 0);
    assert_eq!(run.usage().tool_calls(), 0);
}

#[tokio::test]
async fn missing_or_failing_approval_port_never_starts_tool_execution() {
    for approval_error in [None, Some(ApprovalPortError::Failed)] {
        let contained = Arc::new(FakeContainedToolPort::succeeding(
            definition("write_note", CapabilityKind::LocalWrite),
            ToolOutput::new(serde_json::json!({})),
        ));
        let approval = approval_error.map(|error| {
            Arc::new(ScriptedApprovalPort::scripted(vec![Err(error)])) as Arc<dyn ApprovalPort>
        });
        let runtime = local_write_runtime_with_contained(
            contained.clone(),
            Arc::new(InMemoryAuditSink::new()),
            approval,
        );
        let mut run = context_with_approval_budget(1, 1, 1, Duration::from_secs(30));
        runtime.start_run(&mut run).await.expect("run must start");
        let action = prepare_single_action(&runtime, &mut run).await;

        let error = runtime
            .invoke_validated_action(&mut run, action)
            .await
            .expect_err("missing or failing approval infrastructure must fail closed");

        match approval_error {
            None => {
                assert!(matches!(error, HarnessError::ApprovalPortMissing));
                assert_eq!(run.usage().approval_requests(), 0);
            }
            Some(expected) => {
                assert!(matches!(error, HarnessError::ApprovalPort(actual) if actual == expected));
                assert_eq!(run.usage().approval_requests(), 1);
            }
        }
        assert_eq!(contained.invocation_count(), 0);
        assert_eq!(run.usage().tool_calls(), 0);
    }
}

#[tokio::test]
async fn preview_failures_preserve_category_and_consume_no_execution_budget() {
    for (error, expected_kind) in [
        (
            ContainmentPortError::Unavailable,
            agent_core::ContainmentFailureKind::Unavailable,
        ),
        (
            ContainmentPortError::PreviewRejected,
            agent_core::ContainmentFailureKind::PreviewRejected,
        ),
        (
            ContainmentPortError::Infrastructure,
            agent_core::ContainmentFailureKind::Infrastructure,
        ),
    ] {
        let contained = Arc::new(FakeContainedToolPort::preview_failing(
            definition("write_note", CapabilityKind::LocalWrite),
            error,
        ));
        let audit = Arc::new(InMemoryAuditSink::new());
        let approval = Arc::new(ScriptedApprovalPort::approve_all());
        let runtime = local_write_runtime_with_contained(
            contained.clone(),
            audit.clone(),
            Some(approval.clone()),
        );
        let mut run = context_with_approval_budget(1, 1, 1, Duration::from_secs(30));
        runtime.start_run(&mut run).await.expect("run must start");
        let action = prepare_single_action(&runtime, &mut run).await;

        let actual = runtime
            .invoke_validated_action(&mut run, action)
            .await
            .expect_err("preview failure must fail before approval");

        assert!(matches!(actual, HarnessError::ContainmentPort(actual) if actual == error));
        assert_eq!(approval.invocation_count(), 0);
        assert_eq!(contained.invocation_count(), 0);
        assert_eq!(run.usage().approval_requests(), 0);
        assert_eq!(run.usage().tool_calls(), 0);
        assert!(audit.events().iter().any(|event| matches!(
            event.kind(),
            AgentEventKind::ContainmentFailed { kind, .. } if *kind == expected_kind
        )));
    }
}

#[tokio::test]
async fn approval_requested_audit_failure_consumes_approval_slot_but_invokes_no_port() {
    let approval = Arc::new(ScriptedApprovalPort::approve_all());
    let audit = Arc::new(FailingAuditSink::on_attempt(7));
    let (runtime, contained) =
        local_write_runtime_with_audit(audit, AuditFailurePolicy::FailOpen, approval.clone());
    let mut run = context_with_approval_budget(1, 1, 1, Duration::from_secs(30));
    runtime.start_run(&mut run).await.expect("run must start");
    let action = prepare_single_action(&runtime, &mut run).await;

    let error = runtime
        .invoke_validated_action(&mut run, action)
        .await
        .expect_err("required ApprovalRequested audit must fail closed");

    assert!(matches!(error, HarnessError::Audit { .. }));
    assert_eq!(run.usage().approval_requests(), 1);
    assert_eq!(approval.invocation_count(), 0);
    assert_eq!(contained.invocation_count(), 0);
    assert_eq!(run.usage().tool_calls(), 0);
}

#[tokio::test]
async fn approval_granted_audit_failure_prevents_tool_reservation() {
    let approval = Arc::new(ScriptedApprovalPort::approve_all());
    let audit = Arc::new(FailingAuditSink::on_attempt(8));
    let (runtime, contained) =
        local_write_runtime_with_audit(audit, AuditFailurePolicy::FailOpen, approval.clone());
    let mut run = context_with_approval_budget(1, 1, 1, Duration::from_secs(30));
    runtime.start_run(&mut run).await.expect("run must start");
    let action = prepare_single_action(&runtime, &mut run).await;

    let error = runtime
        .invoke_validated_action(&mut run, action)
        .await
        .expect_err("required ApprovalGranted audit must fail closed");

    assert!(matches!(error, HarnessError::Audit { .. }));
    assert_eq!(approval.invocation_count(), 1);
    assert_eq!(run.usage().approval_requests(), 1);
    assert_eq!(run.usage().tool_calls(), 0);
    assert_eq!(contained.invocation_count(), 0);
}

#[tokio::test]
async fn tool_invocation_started_audit_failure_reserves_tool_but_never_invokes_executor() {
    let approval = Arc::new(ScriptedApprovalPort::approve_all());
    let audit = Arc::new(FailingAuditSink::on_attempt(9));
    let (runtime, contained) =
        local_write_runtime_with_audit(audit, AuditFailurePolicy::FailOpen, approval);
    let mut run = context_with_approval_budget(1, 1, 1, Duration::from_secs(30));
    runtime.start_run(&mut run).await.expect("run must start");
    let action = prepare_single_action(&runtime, &mut run).await;

    let error = runtime
        .invoke_validated_action(&mut run, action)
        .await
        .expect_err("required ToolInvocationStarted audit must fail closed");

    assert!(matches!(error, HarnessError::Audit { .. }));
    assert_eq!(run.usage().approval_requests(), 1);
    assert_eq!(run.usage().tool_calls(), 1);
    assert_eq!(contained.invocation_count(), 0);
}

struct MismatchedApprovalPort {
    mismatch: ApprovalMismatch,
}

#[derive(Clone, Copy)]
enum ApprovalMismatch {
    RequestId,
    Digest,
}

impl ApprovalPort for MismatchedApprovalPort {
    fn decide<'a>(
        &'a self,
        request: &'a crate::ApprovalRequest,
    ) -> PortFuture<'a, Result<ApprovalDecision, ApprovalPortError>> {
        Box::pin(std::future::ready(match self.mismatch {
            ApprovalMismatch::RequestId => Ok(ApprovalDecision::approved(
                agent_core::ApprovalRequestId::new(),
                request.action_digest(),
            )),
            ApprovalMismatch::Digest => Ok(ApprovalDecision::approved(
                request.id(),
                agent_core::ActionDigest::from_bytes([7; 32]),
            )),
        }))
    }
}

#[tokio::test]
async fn approval_decision_must_match_request_id_and_digest() {
    for mismatch in [ApprovalMismatch::RequestId, ApprovalMismatch::Digest] {
        let approval = Arc::new(MismatchedApprovalPort { mismatch });
        let audit = Arc::new(InMemoryAuditSink::new());
        let (runtime, contained) =
            local_write_runtime_with_audit(audit, AuditFailurePolicy::FailClosed, approval);
        let mut run = context_with_approval_budget(1, 1, 1, Duration::from_secs(30));
        runtime.start_run(&mut run).await.expect("run must start");
        let action = prepare_single_action(&runtime, &mut run).await;

        let error = runtime
            .invoke_validated_action(&mut run, action)
            .await
            .expect_err("mismatched approval decision must fail closed");

        assert!(matches!(error, HarnessError::ApprovalDecisionMismatch));
        assert_eq!(run.usage().approval_requests(), 1);
        assert_eq!(run.usage().tool_calls(), 0);
        assert_eq!(contained.invocation_count(), 0);
    }
}

#[tokio::test(start_paused = true)]
async fn pending_approval_observes_cancellation_before_execution() {
    let approval = Arc::new(PendingApprovalPort::new());
    let audit = Arc::new(InMemoryAuditSink::new());
    let (runtime, contained) =
        local_write_runtime_with_audit(audit, AuditFailurePolicy::FailClosed, approval.clone());
    let mut run = context_with_approval_budget(1, 1, 1, Duration::from_secs(30));
    let cancellation = runtime.start_run(&mut run).await.expect("run must start");
    let action = prepare_single_action(&runtime, &mut run).await;
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(1)).await;
        cancellation.request_cancel();
    });

    let error = runtime
        .invoke_validated_action(&mut run, action)
        .await
        .expect_err("pending approval must observe cancellation");

    assert!(matches!(
        error,
        HarnessError::Cancelled {
            stage: ExecutionStage::Invocation
        }
    ));
    assert_eq!(run.status(), &RunStatus::Finished(RunOutcome::Cancelled));
    assert_eq!(approval.invocation_count(), 1);
    assert_eq!(contained.invocation_count(), 0);
}

#[tokio::test(start_paused = true)]
async fn pending_approval_observes_deadline_before_execution() {
    let approval = Arc::new(PendingApprovalPort::new());
    let audit = Arc::new(InMemoryAuditSink::new());
    let (runtime, contained) =
        local_write_runtime_with_audit(audit, AuditFailurePolicy::FailClosed, approval.clone());
    let mut run = context_with_approval_budget(1, 1, 1, Duration::from_secs(1));
    runtime.start_run(&mut run).await.expect("run must start");
    let action = prepare_single_action(&runtime, &mut run).await;

    let error = runtime
        .invoke_validated_action(&mut run, action)
        .await
        .expect_err("pending approval must observe deadline");

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
    assert_eq!(approval.invocation_count(), 1);
    assert_eq!(contained.invocation_count(), 0);
}

fn assert_started_tool_has_terminal_event(events: &[agent_core::AgentEvent]) {
    let started = events.iter().find_map(|event| match event.kind() {
        AgentEventKind::ToolInvocationStarted { tool_call_id, .. } => Some(*tool_call_id),
        _ => None,
    });
    let started = started.expect("contained execution must emit ToolInvocationStarted");
    assert!(events.iter().any(|event| matches!(
        event.kind(),
        AgentEventKind::ToolInvocationCompleted { tool_call_id }
            | AgentEventKind::ToolInvocationDomainFailed { tool_call_id, .. }
            | AgentEventKind::ToolInvocationAdapterFailed { tool_call_id }
            if *tool_call_id == started
    )));
}

#[tokio::test(start_paused = true)]
async fn pending_contained_execution_cancellation_emits_terminal_tool_event() {
    let contained = Arc::new(FakeContainedToolPort::pending(definition(
        "write_note",
        CapabilityKind::LocalWrite,
    )));
    let approval = Arc::new(ScriptedApprovalPort::approve_all());
    let audit = Arc::new(InMemoryAuditSink::new());
    let runtime = local_write_runtime_with_contained(
        contained.clone(),
        audit.clone(),
        Some(approval.clone()),
    );
    let mut run = context_with_approval_budget(1, 1, 1, Duration::from_secs(30));
    let cancellation = runtime.start_run(&mut run).await.expect("run must start");
    let action = prepare_single_action(&runtime, &mut run).await;
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(1)).await;
        cancellation.request_cancel();
    });

    let error = runtime
        .invoke_validated_action(&mut run, action)
        .await
        .expect_err("pending contained execution must observe cancellation");

    assert!(matches!(
        error,
        HarnessError::Cancelled {
            stage: ExecutionStage::Invocation
        }
    ));
    assert_eq!(run.status(), &RunStatus::Finished(RunOutcome::Cancelled));
    assert_eq!(approval.invocation_count(), 1);
    assert_eq!(contained.invocation_count(), 1);
    assert_eq!(contained.termination_count(), 1);
    assert_eq!(run.usage().approval_requests(), 1);
    assert_eq!(run.usage().tool_calls(), 1);
    assert_started_tool_has_terminal_event(&audit.events());
}

#[tokio::test(start_paused = true)]
async fn pending_contained_execution_deadline_emits_terminal_tool_event() {
    let contained = Arc::new(FakeContainedToolPort::pending(definition(
        "write_note",
        CapabilityKind::LocalWrite,
    )));
    let approval = Arc::new(ScriptedApprovalPort::approve_all());
    let audit = Arc::new(InMemoryAuditSink::new());
    let runtime = local_write_runtime_with_contained(
        contained.clone(),
        audit.clone(),
        Some(approval.clone()),
    );
    let mut run = context_with_approval_budget(1, 1, 1, Duration::from_secs(1));
    runtime.start_run(&mut run).await.expect("run must start");
    let action = prepare_single_action(&runtime, &mut run).await;

    let error = runtime
        .invoke_validated_action(&mut run, action)
        .await
        .expect_err("pending contained execution must observe deadline");

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
    assert_eq!(approval.invocation_count(), 1);
    assert_eq!(contained.invocation_count(), 1);
    assert_eq!(contained.termination_count(), 1);
    assert_eq!(run.usage().approval_requests(), 1);
    assert_eq!(run.usage().tool_calls(), 1);
    assert_started_tool_has_terminal_event(&audit.events());
}

#[tokio::test]
async fn contained_tool_result_call_id_mismatch_is_adapter_failure() {
    let model = Arc::new(FakeModelPort::scripted(vec![Ok(response(
        r#"{"action":{"tool":"write_note","arguments":{}}}"#,
    ))]));
    let contained = Arc::new(FakeContainedToolPort::scripted(
        definition("write_note", CapabilityKind::LocalWrite),
        vec![Ok(ToolResult::Succeeded {
            call_id: ToolCallId::new(),
            output: ToolOutput::new(serde_json::json!({})),
        })],
    ));
    let contained_port: Arc<dyn ContainedToolPort> = contained.clone();
    let mut registry = ToolRegistry::new();
    registry
        .register_contained(contained_port)
        .expect("contained local write must register");
    let runtime = harness_with_registry(
        model,
        registry,
        Arc::new(InMemoryAuditSink::new()),
        AuditFailurePolicy::FailClosed,
        Arc::new(M6ApprovalPolicy),
        Some(Arc::new(ScriptedApprovalPort::approve_all())),
    );
    let mut run = context_with_approval_budget(1, 1, 1, Duration::from_secs(30));
    runtime.start_run(&mut run).await.expect("run must start");
    let action = prepare_single_action(&runtime, &mut run).await;

    let error = runtime
        .invoke_validated_action(&mut run, action)
        .await
        .expect_err("call-id mismatch must become adapter failure");

    assert!(matches!(
        error,
        HarnessError::ToolPort(ToolPortError::AdapterFailure)
    ));
    assert_eq!(run.usage().tool_calls(), 1);
    assert_eq!(contained.invocation_count(), 1);
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
        vec![0, 3]
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
