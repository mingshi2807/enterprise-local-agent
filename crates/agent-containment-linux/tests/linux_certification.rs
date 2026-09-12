use std::sync::Arc;
use std::time::Duration;

use agent_action_seal_local::LocalActionSealer;
use agent_containment_linux::{LinuxContainmentConfig, LinuxWorkspaceWriteTool};
use agent_core::{
    AgentEventKind, DurableApprovalOutcome, GraphBranchId, GraphNodeId, GraphRecoveryMode,
    GraphTransitionKey, ModelMessage, ModelOutputPart, ModelRequest, ModelResponse, ModelRole,
    RunBudget, RunId, SessionId, ToolResult, WorkspaceBindingId,
};
use agent_graph::{
    ActionEffects, DecisionContext, Edge, GraphDefinition, GraphFuture, GraphProgram,
    GraphProgramError, GraphTerminalOutcome, ModelEffects, NodeDefinition, NodeKind,
    RestartableGraphProgram, RetrieveEffects, VerificationOutcome, VerifyEffects,
};
use agent_harness::{
    ActionSealPort, AuditFailurePolicy, AuditSink, CompletedModelInvocation, ContainedToolPort,
    DurableApprovalDecisionCommand, DurableApprovalWait, ExecutionHarness, HarnessConfig,
    M6ApprovalPolicy, ManualReconciliationReason, ModelPort, RecoveryDisposition, RunContext,
    RunKey, ToolRegistry,
    testing::{FakeModelPort, InMemoryAuditSink, ScriptedApprovalPort},
};
use serde_json::json;

#[tokio::test]
#[ignore = "run explicitly as the M6.1 Linux security certification"]
async fn production_linux_security_certification() {
    let workspace = tempfile::tempdir().expect("temporary workspace must be created");
    std::fs::create_dir(workspace.path().join("reports"))
        .expect("existing target parent must be created");
    let outside = tempfile::tempdir().expect("outside directory must be created");
    std::os::unix::fs::symlink(outside.path(), workspace.path().join("escape"))
        .expect("escape symlink must be created");

    let bwrap = std::env::var_os("ELA_M6_1_BWRAP")
        .map_or_else(|| std::path::PathBuf::from("/usr/bin/bwrap"), Into::into);
    let worker = std::env::var_os("ELA_M6_1_WORKER").map_or_else(
        || std::path::PathBuf::from(env!("CARGO_BIN_EXE_enterprise-local-write-worker")),
        Into::into,
    );
    let tool = Arc::new(
        LinuxWorkspaceWriteTool::probe_and_create(LinuxContainmentConfig::new(
            workspace.path(),
            bwrap,
            worker,
        ))
        .await
        .expect("mandatory M6.1 host capabilities must be available"),
    );
    eprintln!("M6.1 capabilities: {:?}", tool.capabilities());
    for unsafe_path in ["/tmp/outside", "../outside", "a/../../outside"] {
        let preview = tool.approval_preview(&agent_core::ToolInput::new(json!({
            "relative_path": unsafe_path,
            "content": "blocked"
        })));
        assert!(
            preview.is_err(),
            "unsafe path was previewable: {unsafe_path}"
        );
    }
    tool.certify_process_reaping()
        .await
        .expect("cancellation must kill and reap bwrap, worker, and descendants");

    let first = action("reports/result.txt", "first");
    let second = action("reports/result.txt", "second");
    let escape = action("escape/outside.txt", "blocked");
    let model: Arc<dyn ModelPort> = Arc::new(FakeModelPort::scripted(vec![
        Ok(response(&first)),
        Ok(response(&second)),
        Ok(response(&escape)),
    ]));
    let mut registry = ToolRegistry::new();
    let port: Arc<dyn ContainedToolPort> = tool.clone();
    registry
        .register_contained(port)
        .expect("production contained tool must register");
    let audit = Arc::new(InMemoryAuditSink::new());
    let audit_port: Arc<dyn AuditSink> = audit.clone();
    let persistence_directory = tempfile::tempdir().expect("persistence directory must exist");
    let persistence = Arc::new(
        agent_persistence_sqlite::SqliteRunPersistence::open(
            persistence_directory.path().join("runs.sqlite3"),
        )
        .await
        .expect("SQLite persistence must open"),
    );
    let harness = ExecutionHarness::new(
        model,
        registry,
        Arc::new(M6ApprovalPolicy),
        audit_port,
        HarnessConfig::new(AuditFailurePolicy::FailClosed, Duration::from_secs(1))
            .expect("harness config must be valid"),
    )
    .with_approval_port(Arc::new(ScriptedApprovalPort::scripted(vec![
        Ok(true),
        Ok(true),
        Ok(true),
    ])))
    .with_persistence_port(persistence);
    let budget = RunBudget::new(3, 3, 1, Duration::from_secs(20))
        .expect("budget must be valid")
        .with_max_approval_requests(3);
    let run_id = RunId::new();
    let session_id = SessionId::new();
    let mut context = RunContext::new(run_id, session_id, budget);
    harness
        .start_run(&mut context)
        .await
        .expect("run must start");

    let first_result = execute_action(&harness, &mut context).await;
    assert!(matches!(first_result, ToolResult::Succeeded { .. }));
    assert_eq!(
        std::fs::read_to_string(workspace.path().join("reports/result.txt"))
            .expect("created file must be readable"),
        "first"
    );

    let second_result = execute_action(&harness, &mut context).await;
    assert!(matches!(second_result, ToolResult::Succeeded { .. }));
    assert_eq!(
        std::fs::read_to_string(workspace.path().join("reports/result.txt"))
            .expect("replaced file must be readable"),
        "second"
    );

    let escape_result = execute_action(&harness, &mut context).await;
    assert!(matches!(escape_result, ToolResult::DomainFailure { .. }));
    assert!(!outside.path().join("outside.txt").exists());

    let events = audit.events();
    let started: Vec<_> = events
        .iter()
        .filter_map(|event| match event.kind() {
            AgentEventKind::ToolInvocationStarted { tool_call_id, .. } => Some(*tool_call_id),
            _ => None,
        })
        .collect();
    assert_eq!(started.len(), 3);
    assert_eq!(first_result.call_id(), started[0]);
    assert_eq!(second_result.call_id(), started[1]);
    assert_eq!(escape_result.call_id(), started[2]);
    let serialized = serde_json::to_string(&events).expect("events must serialize");
    assert!(!serialized.contains("first"));
    assert!(!serialized.contains("second"));
    assert!(!serialized.contains("blocked"));

    let recovered = harness
        .recover_run(RunKey::new(run_id, session_id))
        .await
        .expect("governed LocalWrite journal must recover");
    assert!(matches!(
        recovered,
        RecoveryDisposition::ManualReconciliationRequired {
            reason: ManualReconciliationReason::NonRestartableProgram,
            ..
        }
    ));
    assert_eq!(
        std::fs::read_to_string(workspace.path().join("reports/result.txt"))
            .expect("recovery must not alter the target"),
        "second"
    );
    assert!(!outside.path().join("outside.txt").exists());

    certify_durable_local_write(workspace.path(), tool).await;
}

async fn certify_durable_local_write(
    workspace: &std::path::Path,
    tool: Arc<LinuxWorkspaceWriteTool>,
) {
    let definition = durable_definition();
    let persistence_directory = tempfile::tempdir().expect("persistence directory");
    let persistence = Arc::new(
        agent_persistence_sqlite::SqliteRunPersistence::open(
            persistence_directory.path().join("durable.sqlite3"),
        )
        .await
        .expect("SQLite persistence"),
    );
    let seal: Arc<dyn ActionSealPort> =
        Arc::new(LocalActionSealer::new("certification-key", [0x5a; 32]).expect("test key"));
    let workspace_binding = WorkspaceBindingId::new();
    let model = Arc::new(FakeModelPort::scripted(vec![Ok(response(&action(
        "reports/durable.txt",
        "durable",
    )))]));
    let first = durable_harness(
        model,
        tool.clone(),
        persistence.clone(),
        seal.clone(),
        workspace_binding,
    );
    let run_id = RunId::new();
    let session_id = SessionId::new();
    let mut context = RunContext::new_graph(
        run_id,
        session_id,
        RunBudget::new(1, 1, 0, Duration::from_secs(20))
            .expect("budget")
            .with_max_approval_requests(1)
            .with_max_graph_steps(4)
            .expect("graph budget"),
        DurableCertificationProgram::RECOVERY_VERSION,
        definition.digest(),
    );
    first
        .start_run(&mut context)
        .await
        .expect("start durable run");
    let mut program = DurableCertificationProgram::default();
    let waiting = agent_graph::GraphEngine::new(&definition)
        .run(
            &first,
            &mut context,
            &mut program,
            &mut DurableState::default(),
        )
        .await
        .expect("suspend durable action");
    assert_eq!(waiting.terminal(), GraphTerminalOutcome::Waiting);
    let wait = waiting.waiting().expect("wait handle");
    assert_eq!(program.action_calls, 1);
    assert!(!workspace.join("reports/durable.txt").exists());

    let second = durable_harness(
        Arc::new(FakeModelPort::scripted(Vec::new())),
        tool,
        persistence,
        seal,
        workspace_binding,
    );
    let key = RunKey::new(run_id, session_id);
    let view = second
        .durable_approval_view(key, wait.wait_id())
        .await
        .expect("durable preview");
    assert_eq!(view.preview().target_label(), "reports/durable.txt");
    assert!(!view.preview().summary().contains("durable"));
    second
        .record_durable_approval_decision(DurableApprovalDecisionCommand {
            key,
            wait_id: wait.wait_id(),
            approval_request_id: wait.approval_request_id(),
            action_proposal_id: wait.action_proposal_id(),
            tool_call_id: wait.tool_call_id(),
            action_digest: wait.action_digest(),
            expected_row_version: view.row_version(),
            outcome: DurableApprovalOutcome::Approve,
        })
        .await
        .expect("record approval");
    let RecoveryDisposition::Waiting(recovered) =
        second.recover_run(key).await.expect("recover waiting run")
    else {
        panic!("intentional waiting must recover as Waiting");
    };
    let mut resumed_program = DurableCertificationProgram::default();
    let (summary, _) = agent_graph::GraphEngine::new(&definition)
        .resume_waiting(&second, *recovered, wait.wait_id(), &mut resumed_program)
        .await
        .expect("resume durable write");
    assert_eq!(summary.terminal(), GraphTerminalOutcome::Complete);
    assert_eq!(resumed_program.action_calls, 0);
    assert_eq!(
        std::fs::read_to_string(workspace.join("reports/durable.txt")).expect("durable file"),
        "durable"
    );
}

fn durable_harness(
    model: Arc<FakeModelPort>,
    tool: Arc<LinuxWorkspaceWriteTool>,
    persistence: Arc<agent_persistence_sqlite::SqliteRunPersistence>,
    seal: Arc<dyn ActionSealPort>,
    workspace_binding: WorkspaceBindingId,
) -> ExecutionHarness {
    let mut registry = ToolRegistry::new();
    let contained: Arc<dyn ContainedToolPort> = tool;
    registry
        .register_contained(contained)
        .expect("contained tool registration");
    ExecutionHarness::new(
        model,
        registry,
        Arc::new(M6ApprovalPolicy),
        Arc::new(InMemoryAuditSink::new()),
        HarnessConfig::new(AuditFailurePolicy::FailClosed, Duration::from_secs(1))
            .expect("harness config"),
    )
    .with_persistence_port(persistence)
    .with_durable_local_write_approval(seal, workspace_binding)
}

#[derive(Default)]
struct DurableState {
    model: Option<CompletedModelInvocation>,
}

#[derive(Default)]
struct DurableCertificationProgram {
    action_calls: usize,
}

impl GraphProgram for DurableCertificationProgram {
    type WorkingState = DurableState;

    fn retrieve<'a>(
        &'a mut self,
        _node_id: &'a GraphNodeId,
        _state: &'a mut Self::WorkingState,
        _effects: RetrieveEffects<'a>,
    ) -> GraphFuture<'a, Result<(), GraphProgramError>> {
        Box::pin(async { Err(GraphProgramError::Failed) })
    }

    fn model<'a>(
        &'a mut self,
        _node_id: &'a GraphNodeId,
        state: &'a mut Self::WorkingState,
        mut effects: ModelEffects<'a>,
    ) -> GraphFuture<'a, Result<(), GraphProgramError>> {
        Box::pin(async move {
            state.model = Some(
                effects
                    .invoke_model(ModelRequest::new(vec![ModelMessage::new(
                        ModelRole::User,
                        "prepare durable write",
                    )]))
                    .await
                    .map_err(|_| GraphProgramError::Failed)?,
            );
            Ok(())
        })
    }

    fn action<'a>(
        &'a mut self,
        _node_id: &'a GraphNodeId,
        _state: &'a mut Self::WorkingState,
        _effects: ActionEffects<'a>,
    ) -> GraphFuture<'a, Result<(), GraphProgramError>> {
        Box::pin(async { Err(GraphProgramError::Failed) })
    }

    fn durable_local_write_action<'a>(
        &'a mut self,
        _node_id: &'a GraphNodeId,
        state: &'a mut Self::WorkingState,
        mut effects: ActionEffects<'a>,
    ) -> GraphFuture<'a, Result<DurableApprovalWait, GraphProgramError>> {
        self.action_calls += 1;
        Box::pin(async move {
            let model = state.model.take().ok_or(GraphProgramError::Failed)?;
            let action = effects
                .prepare_action(model)
                .await
                .map_err(|_| GraphProgramError::Failed)?;
            effects
                .suspend_local_write(action)
                .await
                .map_err(|_| GraphProgramError::Failed)
        })
    }

    fn verify<'a>(
        &'a mut self,
        _node_id: &'a GraphNodeId,
        _state: &'a mut Self::WorkingState,
        _effects: VerifyEffects<'a>,
    ) -> GraphFuture<'a, Result<VerificationOutcome, GraphProgramError>> {
        Box::pin(async { Ok(VerificationOutcome::Passed) })
    }

    fn decide<'a>(
        &'a mut self,
        _node_id: &'a GraphNodeId,
        _state: &'a mut Self::WorkingState,
        _context: DecisionContext<'a>,
    ) -> GraphFuture<'a, Result<GraphBranchId, GraphProgramError>> {
        Box::pin(async { Err(GraphProgramError::Failed) })
    }
}

impl RestartableGraphProgram for DurableCertificationProgram {
    const RECOVERY_VERSION: u32 = 10;

    fn restore_working_state(
        &mut self,
        _state: &agent_harness::DurableRunState,
    ) -> Result<Self::WorkingState, GraphProgramError> {
        Ok(DurableState::default())
    }
}

fn durable_definition() -> GraphDefinition {
    let id = |value: &str| GraphNodeId::new(value).expect("node ID");
    let node = |value: &str, kind: NodeKind| {
        NodeDefinition::new(id(value), kind, GraphRecoveryMode::Never)
    };
    GraphDefinition::new(
        id("model"),
        vec![
            node("model", NodeKind::Model),
            node("action", NodeKind::DurableLocalWriteAction),
            node("verify", NodeKind::Verify),
            node("complete", NodeKind::Complete),
            node("denied", NodeKind::Fail),
        ],
        vec![
            Edge::new(id("model"), GraphTransitionKey::Succeeded, id("action")),
            Edge::new(id("action"), GraphTransitionKey::Succeeded, id("verify")),
            Edge::new(
                id("action"),
                GraphTransitionKey::ApprovalDenied,
                id("denied"),
            ),
            Edge::new(
                id("verify"),
                GraphTransitionKey::VerificationPassed,
                id("complete"),
            ),
            Edge::new(
                id("verify"),
                GraphTransitionKey::VerificationFailed,
                id("denied"),
            ),
        ],
    )
    .expect("durable graph")
}

async fn execute_action(harness: &ExecutionHarness, context: &mut RunContext) -> ToolResult {
    let invocation = harness
        .invoke_model_tracked(
            context,
            ModelRequest::new(vec![ModelMessage::new(ModelRole::User, "propose action")]),
        )
        .await
        .expect("model invocation must succeed");
    let action = harness
        .prepare_action(context, invocation)
        .await
        .expect("M5 action validation must succeed");
    harness
        .invoke_validated_action(context, action)
        .await
        .expect("M6 governed execution must return a tool result")
}

fn action(relative_path: &str, content: &str) -> String {
    json!({
        "action": {
            "tool": "workspace_write_file",
            "arguments": {
                "relative_path": relative_path,
                "content": content
            }
        }
    })
    .to_string()
}

fn response(text: &str) -> ModelResponse {
    ModelResponse::new(vec![ModelOutputPart::Text(text.to_owned())], None)
}
