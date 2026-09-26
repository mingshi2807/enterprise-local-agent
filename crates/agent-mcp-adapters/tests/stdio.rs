#![cfg(feature = "test-fixture")]

use std::{
    ffi::OsString,
    fs,
    os::unix::fs::PermissionsExt,
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

use agent_core::{
    AgentEventKind, CapabilityKind, ModelOutputPart, ModelResponse, RunBudget, RunId, SessionId,
    ToolCall, ToolCallId, ToolInput, ToolName, ToolResult,
};
use agent_harness::{
    AppendTransition, AuditFailurePolicy, DurableCheckpoint, ExecutionHarness, HarnessConfig,
    LoadedRun, M0ReadOnlyPolicy, M6ApprovalPolicy, ManagedToolPort, ModelPort, PersistenceFuture,
    PersistencePortError, RecoveryDisposition, RunContext, RunKey, RunPersistencePort, RunRecord,
    ToolPortError, ToolRegistry,
    testing::{FakeModelPort, InMemoryAuditSink},
};
use agent_mcp_adapters::{
    DefinitionFingerprint, McpAdapterError, McpServerId, StdioServerConfig, ToolMapping,
    definition_fingerprint, discover_tool,
};
use agent_persistence_sqlite::SqliteRunPersistence;
use serde_json::json;
use tempfile::TempDir;

struct RejectToolStartPersistence;

impl RunPersistencePort for RejectToolStartPersistence {
    fn create_run<'a>(
        &'a self,
        _record: &'a RunRecord,
        _checkpoint: &'a DurableCheckpoint,
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
        _key: RunKey,
    ) -> PersistenceFuture<'a, Result<LoadedRun, PersistencePortError>> {
        Box::pin(std::future::ready(Err(PersistencePortError::Unavailable)))
    }
}

fn fixture_definition(description: &str) -> serde_json::Value {
    json!({
        "name":"remote_lookup",
        "description":description,
        "inputSchema":{
            "type":"object",
            "properties":{"query":{"type":"string","maxLength":128}},
            "required":["query"],
            "additionalProperties":false
        }
    })
}

fn mapping(capability: CapabilityKind) -> ToolMapping {
    ToolMapping::new(
        "remote_lookup",
        ToolName::new("enterprise_lookup").expect("valid local name"),
        "Trusted enterprise lookup",
        capability,
        definition_fingerprint(&fixture_definition("remote")).expect("fixture fingerprint"),
    )
    .expect("valid mapping")
}

fn config(mode: &str, env: Vec<(OsString, OsString)>) -> StdioServerConfig {
    let executable = PathBuf::from(env!("CARGO_BIN_EXE_mcp-fixture-server"));
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o755))
        .expect("fixture permissions are trusted");
    StdioServerConfig::new(
        McpServerId::new("fixture").expect("valid server id"),
        executable,
        vec![OsString::from(mode)],
        env,
        Duration::from_secs(2),
    )
    .expect("valid fixture config")
}

fn call() -> ToolCall {
    ToolCall::new(
        ToolCallId::new(),
        ToolName::new("enterprise_lookup").expect("valid name"),
        ToolInput::new(json!({"query":"status"})),
    )
}

#[tokio::test]
async fn pinned_protocol_discovery_and_successful_call() {
    let tool = discover_tool(config("normal", vec![]), mapping(CapabilityKind::ReadOnly))
        .await
        .expect("discovery succeeds");
    assert_eq!(tool.definition().description(), "Trusted enterprise lookup");
    let mut invocation = tool.start_managed(call()).expect("managed start succeeds");
    let result = invocation.wait().await.expect("call succeeds");
    assert!(matches!(result, ToolResult::Succeeded { .. }));
}

#[tokio::test]
async fn model_to_m5_policy_to_managed_mcp_readonly_flow_preserves_ids() {
    let tool = Arc::new(
        discover_tool(config("normal", vec![]), mapping(CapabilityKind::ReadOnly))
            .await
            .expect("discovery succeeds"),
    );
    let mut registry = ToolRegistry::new();
    let managed: Arc<dyn ManagedToolPort> = tool;
    registry
        .register_managed(managed)
        .expect("managed definition registers");
    let model = Arc::new(FakeModelPort::scripted(vec![Ok(ModelResponse::new(
        vec![ModelOutputPart::Text(
            r#"{"action":{"tool":"enterprise_lookup","arguments":{"query":"status"}}}"#.to_owned(),
        )],
        None,
    ))]));
    let model_port: Arc<dyn ModelPort> = model;
    let audit = Arc::new(InMemoryAuditSink::new());
    let harness = ExecutionHarness::new(
        model_port,
        registry,
        Arc::new(M6ApprovalPolicy),
        audit.clone(),
        HarnessConfig::new(AuditFailurePolicy::FailClosed, Duration::from_secs(1))
            .expect("valid harness config"),
    );
    let mut run = RunContext::new(
        RunId::new(),
        SessionId::new(),
        RunBudget::new(1, 1, 1, Duration::from_secs(10)).expect("valid budget"),
    );
    harness.start_run(&mut run).await.expect("run starts");
    let invocation = harness
        .invoke_model_tracked(
            &mut run,
            agent_core::ModelRequest::new(vec![agent_core::ModelMessage::new(
                agent_core::ModelRole::User,
                "lookup",
            )]),
        )
        .await
        .expect("model invocation succeeds");
    let action = harness
        .prepare_action(&mut run, invocation)
        .await
        .expect("M5 validates action");
    let result = harness
        .invoke_validated_action(&mut run, action)
        .await
        .expect("governed MCP invocation succeeds");
    let call_id = result.call_id();
    assert!(audit.events().iter().any(|event| matches!(
        event.kind(),
        AgentEventKind::ToolInvocationStarted { tool_call_id, .. }
            if *tool_call_id == call_id
    )));
    assert!(audit.events().iter().any(|event| matches!(
        event.kind(),
        AgentEventKind::ToolInvocationCompleted { tool_call_id }
            if *tool_call_id == call_id
    )));
}

#[tokio::test]
async fn durable_start_failure_dispatches_zero_mcp_calls() {
    let temp = TempDir::new().expect("tempdir");
    let calls = temp.path().join("calls");
    let tool = Arc::new(
        discover_tool(
            config(
                "normal",
                vec![(
                    OsString::from("MCP_FIXTURE_CALLS"),
                    calls.as_os_str().to_owned(),
                )],
            ),
            mapping(CapabilityKind::ReadOnly),
        )
        .await
        .expect("discovery succeeds"),
    );
    let mut registry = ToolRegistry::new();
    let managed: Arc<dyn ManagedToolPort> = tool;
    registry
        .register_managed(managed)
        .expect("managed definition registers");
    let model: Arc<dyn ModelPort> = Arc::new(FakeModelPort::scripted(vec![]));
    let runtime = ExecutionHarness::new(
        model,
        registry,
        Arc::new(M0ReadOnlyPolicy),
        Arc::new(InMemoryAuditSink::new()),
        HarnessConfig::new(AuditFailurePolicy::FailClosed, Duration::from_secs(1))
            .expect("valid harness config"),
    )
    .with_persistence_port(Arc::new(RejectToolStartPersistence));
    let mut run = RunContext::new(
        RunId::new(),
        SessionId::new(),
        RunBudget::new(0, 1, 1, Duration::from_secs(10)).expect("valid budget"),
    );
    runtime.start_run(&mut run).await.expect("run starts");
    assert!(matches!(
        runtime.invoke_tool(&mut run, call()).await,
        Err(agent_harness::HarnessError::Persistence(
            PersistencePortError::Unavailable
        ))
    ));
    assert!(
        !calls.exists(),
        "tools/call must not follow failed persistence"
    );
}

#[tokio::test]
async fn protocol_mismatch_bad_id_duplicate_and_cursor_loop_fail_closed() {
    assert!(matches!(
        discover_tool(
            config("version-mismatch", vec![]),
            mapping(CapabilityKind::ReadOnly)
        )
        .await,
        Err(McpAdapterError::ProtocolMismatch)
    ));
    assert!(matches!(
        discover_tool(config("bad-id", vec![]), mapping(CapabilityKind::ReadOnly)).await,
        Err(McpAdapterError::MalformedResponse)
    ));
    assert!(matches!(
        discover_tool(
            config("duplicate", vec![]),
            mapping(CapabilityKind::ReadOnly)
        )
        .await,
        Err(McpAdapterError::AmbiguousDiscovery)
    ));
    assert!(matches!(
        discover_tool(
            config("cursor-loop", vec![]),
            mapping(CapabilityKind::ReadOnly)
        )
        .await,
        Err(McpAdapterError::AmbiguousDiscovery)
    ));
}

#[tokio::test]
async fn oversized_stdio_frame_is_rejected_before_json_parsing() {
    let bounded = config("oversized-list", vec![]).with_limits(1024, 1024, 2, 8);
    assert!(matches!(
        discover_tool(bounded, mapping(CapabilityKind::ReadOnly)).await,
        Err(McpAdapterError::MalformedResponse)
    ));
}

#[tokio::test]
async fn unsupported_m5_schema_is_not_treated_as_malformed_mcp() {
    let unsupported = json!({
        "name":"remote_lookup", "description":"remote",
        "inputSchema":{"type":"object","oneOf":[{"type":"object"}]}
    });
    let mapping = ToolMapping::new(
        "remote_lookup",
        ToolName::new("enterprise_lookup").expect("valid name"),
        "Trusted enterprise lookup",
        CapabilityKind::ReadOnly,
        definition_fingerprint(&unsupported).expect("fixture fingerprint"),
    )
    .expect("valid mapping");
    assert!(matches!(
        discover_tool(config("unsupported-schema", vec![]), mapping).await,
        Err(McpAdapterError::UnsupportedTool)
    ));
}

#[tokio::test]
async fn invocation_time_definition_drift_prevents_remote_dispatch() {
    let temp = TempDir::new().expect("tempdir");
    let state = temp.path().join("state");
    let calls = temp.path().join("calls");
    let environment = vec![
        (
            OsString::from("MCP_FIXTURE_STATE"),
            state.as_os_str().to_owned(),
        ),
        (
            OsString::from("MCP_FIXTURE_CALLS"),
            calls.as_os_str().to_owned(),
        ),
    ];
    let tool = discover_tool(
        config("drift", environment),
        mapping(CapabilityKind::ReadOnly),
    )
    .await
    .expect("initial snapshot matches");
    let mut invocation = tool.start_managed(call()).expect("managed start succeeds");
    assert_eq!(invocation.wait().await, Err(ToolPortError::AdapterFailure));
    assert!(!calls.exists(), "tools/call must not be sent after drift");
}

#[tokio::test]
async fn configured_non_readonly_capabilities_never_spawn_or_dispatch() {
    for capability in [
        CapabilityKind::LocalWrite,
        CapabilityKind::ExternalWrite,
        CapabilityKind::Privileged,
    ] {
        let temp = TempDir::new().expect("tempdir");
        let calls = temp.path().join("calls");
        let tool = discover_tool(
            config(
                "normal",
                vec![(
                    OsString::from("MCP_FIXTURE_CALLS"),
                    calls.as_os_str().to_owned(),
                )],
            ),
            mapping(capability),
        )
        .await
        .expect("definition discovery is capability-neutral");
        assert!(matches!(
            tool.start_managed(call()),
            Err(ToolPortError::AdapterFailure)
        ));
        assert!(!calls.exists());
    }
}

#[tokio::test]
async fn domain_and_unsupported_result_forms_are_distinct() {
    let domain = discover_tool(
        config("domain-error", vec![]),
        mapping(CapabilityKind::ReadOnly),
    )
    .await
    .expect("discovery succeeds");
    let call_id = call().id();
    let mut invocation = domain
        .start_managed(ToolCall::new(
            call_id,
            ToolName::new("enterprise_lookup").expect("valid name"),
            ToolInput::new(json!({"query":"status"})),
        ))
        .expect("start succeeds");
    assert!(matches!(
        invocation.wait().await,
        Ok(ToolResult::DomainFailure { call_id: actual, .. }) if actual == call_id
    ));

    let rich = discover_tool(config("rich", vec![]), mapping(CapabilityKind::ReadOnly))
        .await
        .expect("discovery succeeds");
    let mut invocation = rich.start_managed(call()).expect("start succeeds");
    assert_eq!(invocation.wait().await, Err(ToolPortError::AdapterFailure));

    let malformed = discover_tool(
        config("malformed-is-error", vec![]),
        mapping(CapabilityKind::ReadOnly),
    )
    .await
    .expect("discovery succeeds");
    let mut invocation = malformed.start_managed(call()).expect("start succeeds");
    assert_eq!(invocation.wait().await, Err(ToolPortError::AdapterFailure));
}

#[tokio::test]
async fn invocation_timeout_is_sanitized_and_process_is_reaped() {
    let tool = discover_tool(config("hang", vec![]), mapping(CapabilityKind::ReadOnly))
        .await
        .expect("discovery succeeds");
    let mut invocation = tool.start_managed(call()).expect("start succeeds");
    assert_eq!(invocation.wait().await, Err(ToolPortError::AdapterFailure));
    invocation
        .terminate_and_reap()
        .await
        .expect("already-reaped invocation is idempotent");
}

#[tokio::test]
async fn recovery_replay_performs_zero_mcp_operations() {
    let temp = TempDir::new().expect("tempdir");
    let calls = temp.path().join("calls");
    let tool = Arc::new(
        discover_tool(
            config(
                "hang",
                vec![(
                    OsString::from("MCP_FIXTURE_CALLS"),
                    calls.as_os_str().to_owned(),
                )],
            ),
            mapping(CapabilityKind::ReadOnly),
        )
        .await
        .expect("discovery succeeds"),
    );
    let persistence = Arc::new(
        SqliteRunPersistence::open(temp.path().join("runs.sqlite3"))
            .await
            .expect("SQLite opens"),
    );
    let mut registry = ToolRegistry::new();
    let managed: Arc<dyn ManagedToolPort> = tool;
    registry
        .register_managed(managed)
        .expect("managed definition registers");
    let model: Arc<dyn ModelPort> = Arc::new(FakeModelPort::scripted(vec![]));
    let runtime = Arc::new(
        ExecutionHarness::new(
            model,
            registry,
            Arc::new(M0ReadOnlyPolicy),
            Arc::new(InMemoryAuditSink::new()),
            HarnessConfig::new(AuditFailurePolicy::FailClosed, Duration::from_secs(1))
                .expect("valid harness config"),
        )
        .with_persistence_port(persistence.clone()),
    );
    let mut run = RunContext::new(
        RunId::new(),
        SessionId::new(),
        RunBudget::new(0, 1, 1, Duration::from_secs(20)).expect("valid budget"),
    );
    let key = RunKey::new(run.run_id(), run.session_id());
    runtime.start_run(&mut run).await.expect("run starts");
    let task_runtime = Arc::clone(&runtime);
    let task = tokio::spawn(async move {
        task_runtime
            .invoke_tool(
                &mut run,
                ToolCall::new(
                    ToolCallId::new(),
                    ToolName::new("enterprise_lookup").expect("valid name"),
                    ToolInput::new(json!({"query":"status"})),
                ),
            )
            .await
    });
    let deadline = Instant::now() + Duration::from_secs(2);
    while !calls.exists() && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(calls.exists(), "fixture must observe one tools/call");
    task.abort();
    let _ = task.await;
    let before = fs::read_to_string(&calls).expect("call marker");

    let recovery_model: Arc<dyn ModelPort> = Arc::new(FakeModelPort::scripted(vec![]));
    let recovery = ExecutionHarness::new(
        recovery_model,
        ToolRegistry::new(),
        Arc::new(M0ReadOnlyPolicy),
        Arc::new(InMemoryAuditSink::new()),
        HarnessConfig::new(AuditFailurePolicy::FailClosed, Duration::from_secs(1))
            .expect("valid harness config"),
    )
    .with_persistence_port(persistence);
    assert!(matches!(
        recovery.recover_run(key).await,
        Ok(RecoveryDisposition::ManualReconciliationRequired { .. })
    ));
    assert_eq!(fs::read_to_string(calls).expect("call marker"), before);
}

#[tokio::test]
async fn termination_reaps_process_group_descendant() {
    let temp = TempDir::new().expect("tempdir");
    for cycle in 0..8 {
        let pid_file = temp.path().join(format!("descendant-{cycle}.pid"));
        let tool = discover_tool(
            config(
                "descendant",
                vec![(
                    OsString::from("MCP_FIXTURE_DESCENDANT_PID"),
                    pid_file.as_os_str().to_owned(),
                )],
            ),
            mapping(CapabilityKind::ReadOnly),
        )
        .await
        .expect("discovery succeeds");
        let mut invocation = tool.start_managed(call()).expect("start succeeds");
        assert!(
            tokio::time::timeout(Duration::from_millis(300), invocation.wait())
                .await
                .is_err()
        );
        let deadline = Instant::now() + Duration::from_secs(2);
        while !pid_file.exists() && Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let pid = fs::read_to_string(&pid_file)
            .expect("fixture wrote descendant pid")
            .trim()
            .to_owned();
        let proc_path = PathBuf::from(format!("/proc/{pid}"));
        let before = read_process_stat(&pid).expect("live descendant process stat");
        eprintln!(
            "descendant before cleanup: cycle={cycle} pid={pid} ppid={} pgid={} session={} state={} start_time={}",
            before.parent_pid,
            before.process_group,
            before.session,
            before.state,
            before.start_time
        );

        invocation
            .terminate_and_reap()
            .await
            .expect("process group reaped");
        while process_is_live(&pid, before.start_time) && Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        if proc_path.exists()
            && let Some(after) = read_process_stat(&pid)
        {
            eprintln!(
                "descendant after cleanup: cycle={cycle} pid={pid} ppid={} pgid={} session={} state={} start_time={}",
                after.parent_pid, after.process_group, after.session, after.state, after.start_time
            );
            assert_eq!(
                after.start_time, before.start_time,
                "descendant PID was reused"
            );
            assert_eq!(
                after.state, 'Z',
                "a live managed descendant survived process-group cleanup"
            );
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct ProcessStat {
    state: char,
    parent_pid: u32,
    process_group: u32,
    session: u32,
    start_time: u64,
}

fn read_process_stat(pid: &str) -> Option<ProcessStat> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let fields = stat
        .rsplit_once(')')?
        .1
        .split_whitespace()
        .collect::<Vec<_>>();
    Some(ProcessStat {
        state: fields.first()?.chars().next()?,
        parent_pid: fields.get(1)?.parse().ok()?,
        process_group: fields.get(2)?.parse().ok()?,
        session: fields.get(3)?.parse().ok()?,
        start_time: fields.get(19)?.parse().ok()?,
    })
}

fn process_is_live(pid: &str, expected_start_time: u64) -> bool {
    read_process_stat(pid)
        .is_some_and(|stat| stat.start_time == expected_start_time && stat.state != 'Z')
}

#[test]
fn fingerprint_type_does_not_expose_digest_in_debug() {
    assert_eq!(
        format!("{:?}", DefinitionFingerprint::from_bytes([7; 32])),
        "DefinitionFingerprint([REDACTED])"
    );
}
