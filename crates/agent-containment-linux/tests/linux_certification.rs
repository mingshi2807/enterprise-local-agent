use std::sync::Arc;
use std::time::Duration;

use agent_containment_linux::{LinuxContainmentConfig, LinuxWorkspaceWriteTool};
use agent_core::{
    AgentEventKind, ModelMessage, ModelOutputPart, ModelRequest, ModelResponse, ModelRole,
    RunBudget, RunId, SessionId, ToolResult,
};
use agent_harness::{
    AuditFailurePolicy, AuditSink, ContainedToolPort, ExecutionHarness, HarnessConfig,
    M6ApprovalPolicy, ModelPort, RunContext, ToolRegistry,
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
    let port: Arc<dyn ContainedToolPort> = tool;
    registry
        .register_contained(port)
        .expect("production contained tool must register");
    let audit = Arc::new(InMemoryAuditSink::new());
    let audit_port: Arc<dyn AuditSink> = audit.clone();
    let harness = ExecutionHarness::new(
        model,
        registry,
        Arc::new(M6ApprovalPolicy),
        audit_port,
        HarnessConfig::new(AuditFailurePolicy::FailClosed, Duration::from_secs(1))
            .expect("harness config must be valid"),
    )
    .with_approval_port(Arc::new(ScriptedApprovalPort::approve_all()));
    let budget = RunBudget::new(3, 3, 1, Duration::from_secs(20))
        .expect("budget must be valid")
        .with_max_approval_requests(3);
    let mut context = RunContext::new(RunId::new(), SessionId::new(), budget);
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
