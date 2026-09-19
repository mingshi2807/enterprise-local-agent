use std::{
    env,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use agent_action_seal_local::LocalActionSealer;
use agent_containment_linux::{LinuxContainmentConfig, LinuxWorkspaceWriteTool};
use agent_core::{
    KnowledgeBackendId, ModelOutputPart, ModelRequest, ModelResponse, ToolCall, ToolDefinition,
    ToolInput, WorkspaceBindingId,
};
use agent_harness::{
    ApprovalPreview, AuditFailurePolicy, AuditSink, ContainedInvocation, ContainedToolPort,
    ContainmentPortError, ExecutionHarness, HarnessConfig, M6ApprovalPolicy, ModelPort,
    ModelPortError, PortFuture, RunPersistencePort, ToolRegistry, testing::InMemoryAuditSink,
};
use agent_knowledge::{
    EvidenceSet, KnowledgeBackendPort, KnowledgeError, KnowledgeFuture, KnowledgePort,
    KnowledgeRequest, KnowledgeRoute, RoutedKnowledgePort,
};
use agent_knowledge_adapters::{
    OcppApiConfig, OcppKnowledgeAdapter, StandardsMcpConfig, StandardsMcpKnowledgeAdapter,
};
use agent_mvp::{EnterpriseMvpWorkflow, LOCALWRITE_WORKFLOW_ID, READONLY_WORKFLOW_ID};
use agent_persistence_sqlite::SqliteRunPersistence;
use agent_provider_rig::{
    BearerCredential, OpenAiCompatibleConfig, build_openai_compatible_model_port,
    probe_openai_compatible,
};
use agent_service::{
    AgentService, ApplicationResultV1, ApprovalDecisionV1, ConfiguredWorkflow, RunDispositionV1,
    RunInput, RunKey, RunView, WorkflowId,
};
use uuid::Uuid;

fn required(name: &str) -> String {
    env::var(name).unwrap_or_else(|_| panic!("{name} is required for the opted-in smoke test"))
}

fn model_config() -> OpenAiCompatibleConfig {
    let endpoint = required("ELA_SMOKE_MODEL_BASE_URL");
    let model = required("ELA_SMOKE_MODEL_ID");
    let config = match required("ELA_SMOKE_MODEL_AUTH").as_str() {
        "no-auth-loopback" => OpenAiCompatibleConfig::new_no_auth_loopback(endpoint, model)
            .expect("explicit loopback no-auth model configuration"),
        "bearer" => OpenAiCompatibleConfig::new(
            endpoint,
            model,
            BearerCredential::new(required("ELA_SMOKE_MODEL_BEARER")).expect("bearer"),
        )
        .expect("bearer model configuration"),
        _ => panic!("ELA_SMOKE_MODEL_AUTH must be bearer or no-auth-loopback"),
    };
    config
        .with_json_object_output(256)
        .expect("bounded JSON model output")
        .with_reasoning_disabled()
}

struct ObservedModel {
    inner: Arc<dyn ModelPort>,
    invocations: AtomicUsize,
    last_shape: Mutex<Option<String>>,
}

impl ObservedModel {
    fn new(inner: Arc<dyn ModelPort>) -> Arc<Self> {
        Arc::new(Self {
            inner,
            invocations: AtomicUsize::new(0),
            last_shape: Mutex::new(None),
        })
    }

    fn invocations(&self) -> usize {
        self.invocations.load(Ordering::SeqCst)
    }

    fn last_shape(&self) -> Option<String> {
        self.last_shape.lock().ok().and_then(|value| value.clone())
    }
}

impl ModelPort for ObservedModel {
    fn invoke<'a>(
        &'a self,
        request: ModelRequest,
    ) -> PortFuture<'a, Result<ModelResponse, ModelPortError>> {
        self.invocations.fetch_add(1, Ordering::SeqCst);
        Box::pin(async move {
            let result = self.inner.invoke(request).await;
            if let Ok(response) = &result {
                let shape = sanitized_model_shape(response);
                if let Ok(mut stored) = self.last_shape.lock() {
                    *stored = Some(shape.clone());
                }
                eprintln!("M13 model output shape: {shape}");
            }
            result
        })
    }
}

fn sanitized_model_shape(response: &ModelResponse) -> String {
    match response.output() {
        [ModelOutputPart::Text(text)] => {
            let trimmed = text.trim();
            let parsed = serde_json::from_str::<serde_json::Value>(trimmed).ok();
            let mut keys = parsed
                .as_ref()
                .and_then(serde_json::Value::as_object)
                .map(|object| object.keys().cloned().collect::<Vec<_>>())
                .unwrap_or_default();
            keys.sort();
            let citation_ids = parsed
                .as_ref()
                .and_then(|value| value.get("citations"))
                .and_then(serde_json::Value::as_array)
                .map(|values| {
                    values
                        .iter()
                        .filter_map(serde_json::Value::as_str)
                        .map(str::to_owned)
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            format!(
                "single_text bytes={} json={} root_keys={keys:?} citation_ids={citation_ids:?} fenced={} think_tag={}",
                text.len(),
                parsed.is_some(),
                trimmed.starts_with("```") || trimmed.ends_with("```"),
                trimmed.contains("<think>") || trimmed.contains("</think>")
            )
        }
        output => format!("parts={} non_text_or_mixed=true", output.len()),
    }
}

struct ObservedKnowledge {
    inner: Arc<dyn KnowledgePort>,
    invocations: AtomicUsize,
}

impl ObservedKnowledge {
    fn new(inner: Arc<dyn KnowledgePort>) -> Arc<Self> {
        Arc::new(Self {
            inner,
            invocations: AtomicUsize::new(0),
        })
    }

    fn invocations(&self) -> usize {
        self.invocations.load(Ordering::SeqCst)
    }
}

impl KnowledgePort for ObservedKnowledge {
    fn retrieve<'a>(
        &'a self,
        request: KnowledgeRequest,
    ) -> KnowledgeFuture<'a, Result<EvidenceSet, KnowledgeError>> {
        self.invocations.fetch_add(1, Ordering::SeqCst);
        Box::pin(async move {
            let result = self.inner.retrieve(request).await;
            if let Ok(evidence) = &result {
                eprintln!(
                    "M13 retrieval shape: evidence={} bytes={} degraded={} truncated={}",
                    evidence.evidence().len(),
                    evidence.total_content_bytes(),
                    evidence.degraded(),
                    evidence.truncated()
                );
            }
            result
        })
    }
}

struct ObservedContainedTool {
    inner: Arc<dyn ContainedToolPort>,
    dispatches: Arc<AtomicUsize>,
}

impl ObservedContainedTool {
    fn new(inner: Arc<dyn ContainedToolPort>, dispatches: Arc<AtomicUsize>) -> Arc<Self> {
        Arc::new(Self { inner, dispatches })
    }
}

impl ContainedToolPort for ObservedContainedTool {
    fn definition(&self) -> &ToolDefinition {
        self.inner.definition()
    }

    fn approval_preview(&self, input: &ToolInput) -> Result<ApprovalPreview, ContainmentPortError> {
        self.inner.approval_preview(input)
    }

    fn start_contained(
        &self,
        call: ToolCall,
    ) -> Result<Box<dyn ContainedInvocation>, ContainmentPortError> {
        self.dispatches.fetch_add(1, Ordering::SeqCst);
        self.inner.start_contained(call)
    }
}

fn knowledge() -> (Arc<dyn KnowledgePort>, KnowledgeRoute) {
    let (backend, backend_id): (Arc<dyn KnowledgeBackendPort>, KnowledgeBackendId) =
        match required("ELA_SMOKE_KNOWLEDGE_ROUTE").as_str() {
            "ocpp" => {
                let config = OcppApiConfig::new(
                    url::Url::parse(&required("ELA_SMOKE_OCPP_URL")).expect("OCPP URL"),
                )
                .expect("OCPP configuration");
                (
                    Arc::new(OcppKnowledgeAdapter::new(config).expect("OCPP adapter")),
                    KnowledgeBackendId::OcppRagKag,
                )
            }
            "standards" => {
                let arguments: Vec<String> =
                    serde_json::from_str(&required("ELA_SMOKE_STANDARDS_ARGV_JSON"))
                        .expect("standards argv JSON");
                let environment: Vec<(String, String)> = serde_json::from_str(
                    &env::var("ELA_SMOKE_STANDARDS_ENV_JSON").unwrap_or_else(|_| "[]".to_owned()),
                )
                .expect("standards environment JSON");
                let config = StandardsMcpConfig::new(
                    PathBuf::from(required("ELA_SMOKE_STANDARDS_EXECUTABLE")),
                    arguments.into_iter().map(Into::into).collect(),
                    environment
                        .into_iter()
                        .map(|(key, value)| (key.into(), value.into()))
                        .collect(),
                )
                .expect("standards MCP configuration");
                (
                    Arc::new(StandardsMcpKnowledgeAdapter::new(config)),
                    KnowledgeBackendId::StandardsMcp,
                )
            }
            _ => panic!("ELA_SMOKE_KNOWLEDGE_ROUTE must be ocpp or standards"),
        };
    (
        Arc::new(RoutedKnowledgePort::new(vec![backend]).expect("knowledge router")),
        KnowledgeRoute::single(backend_id),
    )
}

async fn persistence() -> (tempfile::TempDir, Arc<SqliteRunPersistence>) {
    let directory = tempfile::tempdir().expect("temporary service data");
    let store = Arc::new(
        SqliteRunPersistence::open(directory.path().join("runs.sqlite3"))
            .await
            .expect("SQLite persistence"),
    );
    (directory, store)
}

fn base_harness(
    model: Arc<dyn agent_harness::ModelPort>,
    tools: ToolRegistry,
    knowledge: Arc<dyn KnowledgePort>,
    store: Arc<dyn RunPersistencePort>,
) -> ExecutionHarness {
    ExecutionHarness::new(
        model,
        tools,
        Arc::new(M6ApprovalPolicy),
        Arc::new(InMemoryAuditSink::new()) as Arc<dyn AuditSink>,
        HarnessConfig::new(AuditFailurePolicy::FailClosed, Duration::from_secs(90))
            .expect("harness config"),
    )
    .with_persistence_port(store)
    .with_knowledge_port(knowledge)
}

async fn report_run(service: &Arc<AgentService>, key: RunKey, view: &RunView) {
    eprintln!(
        "M13 run: run_id={} status={:?} outcome={:?} duration_ms={:?} result={:?}",
        view.run_id, view.disposition, view.outcome, view.duration_millis, view.result
    );
    if let Ok(events) = service.read_events(key, None, 128).await {
        for event in events {
            eprintln!(
                "M13 event: seq={} category={:?} phase={:?} correlation={:?} backend={:?} node={:?} budget={:?}/{:?}",
                event.sequence,
                event.category,
                event.phase,
                event.correlation_id,
                event.knowledge_backends,
                event.graph_node_id,
                event.budget_usage,
                event.budget_limit
            );
        }
    }
}

async fn wait_for(service: &Arc<AgentService>, key: RunKey, expected: RunDispositionV1) -> RunView {
    for _ in 0..900 {
        if let Ok(view) = service.get_run_status(key).await {
            if view.disposition == expected {
                report_run(service, key, &view).await;
                return view;
            }
            if matches!(
                view.disposition,
                RunDispositionV1::Completed
                    | RunDispositionV1::Failed
                    | RunDispositionV1::ManualReconciliationRequired
            ) {
                report_run(service, key, &view).await;
                panic!(
                    "real smoke run reached {:?}, expected {expected:?}",
                    view.disposition
                );
            }
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("real smoke run did not reach {expected:?}");
}

#[tokio::test]
#[ignore = "requires explicit real local model and enterprise knowledge configuration"]
async fn real_readonly_knowledge_to_local_model_answer() {
    let config = model_config();
    probe_openai_compatible(&config)
        .await
        .expect("model readiness");
    let model = ObservedModel::new(build_openai_compatible_model_port(config).expect("model"));
    let (knowledge, route) = knowledge();
    let knowledge = ObservedKnowledge::new(knowledge);
    let (_directory, store) = persistence().await;
    let harness = Arc::new(base_harness(
        model.clone(),
        ToolRegistry::new(),
        knowledge.clone(),
        store.clone(),
    ));
    let workflow: Arc<dyn ConfiguredWorkflow> =
        Arc::new(EnterpriseMvpWorkflow::readonly(route).expect("workflow"));
    let service = Arc::new(AgentService::new(harness, store, vec![workflow]).expect("service"));
    let session = service.create_session().await.expect("session");
    let run = service
        .start_run(
            session,
            Uuid::new_v4(),
            &WorkflowId::new(READONLY_WORKFLOW_ID).expect("workflow id"),
            RunInput::new(required("ELA_SMOKE_READONLY_INPUT").into_bytes()).expect("input"),
        )
        .await
        .expect("start");
    let key = RunKey::new(run.run_id, session);
    let view = wait_for(&service, key, RunDispositionV1::Completed).await;
    let Some(ApplicationResultV1::FinalAnswer { answer, citations }) = view.result else {
        panic!("readonly run did not produce a final answer");
    };
    assert!(!answer.is_empty());
    assert!(!citations.is_empty());
    assert!(
        citations
            .iter()
            .all(|citation| citation.backend == KnowledgeBackendId::StandardsMcp)
    );
    assert_eq!(model.invocations(), 1);
    assert_eq!(knowledge.invocations(), 1);
}

async fn real_localwrite(decision: ApprovalDecisionV1) {
    let config = model_config();
    probe_openai_compatible(&config)
        .await
        .expect("model readiness");
    let model = ObservedModel::new(build_openai_compatible_model_port(config).expect("model"));
    let (knowledge, route) = knowledge();
    let knowledge = ObservedKnowledge::new(knowledge);
    let (_directory, store) = persistence().await;
    let workspace = tempfile::tempdir().expect("disposable write workspace");
    let target = required("ELA_SMOKE_WRITE_RELATIVE_PATH");
    assert!(!PathBuf::from(&target).is_absolute());
    let tool: Arc<dyn ContainedToolPort> = Arc::new(
        LinuxWorkspaceWriteTool::probe_and_create(LinuxContainmentConfig::new(
            workspace.path(),
            PathBuf::from(required("ELA_SMOKE_BWRAP_PATH")),
            PathBuf::from(required("ELA_SMOKE_WORKER_PATH")),
        ))
        .await
        .expect("M6.1 containment readiness"),
    );
    let dispatches = Arc::new(AtomicUsize::new(0));
    let mut tools = ToolRegistry::new();
    let contained: Arc<dyn ContainedToolPort> =
        ObservedContainedTool::new(tool, dispatches.clone());
    tools.register_contained(contained).expect("contained tool");
    let seal = Arc::new(LocalActionSealer::new("m13-real-smoke", [0x73; 32]).expect("seal"));
    let binding = WorkspaceBindingId::new();
    let harness = Arc::new(
        base_harness(model.clone(), tools, knowledge.clone(), store.clone())
            .with_durable_local_write_approval(seal.clone(), binding),
    );
    let workflow: Arc<dyn ConfiguredWorkflow> =
        Arc::new(EnterpriseMvpWorkflow::localwrite(route.clone()).expect("workflow"));
    let service =
        Arc::new(AgentService::new(harness, store.clone(), vec![workflow]).expect("service"));
    let session = service.create_session().await.expect("session");
    let prompt = format!(
        "{} Write exactly to relative path {target}.",
        required("ELA_SMOKE_WRITE_INPUT")
    );
    let run = service
        .start_run(
            session,
            Uuid::new_v4(),
            &WorkflowId::new(LOCALWRITE_WORKFLOW_ID).expect("workflow id"),
            RunInput::new(prompt.into_bytes()).expect("input"),
        )
        .await
        .expect("start");
    let key = RunKey::new(run.run_id, session);
    wait_for(&service, key, RunDispositionV1::Waiting).await;
    let (waits, _) = service.waiting_page(None, 8).await.expect("waiting");
    let wait = waits.first().expect("wait").clone();
    let preview = service
        .approval_preview(key, wait.wait_id)
        .await
        .expect("preview");
    assert_eq!(preview.target, target);
    assert!(preview.summary.len() <= 512);
    drop(service);

    let tool: Arc<dyn ContainedToolPort> = Arc::new(
        LinuxWorkspaceWriteTool::probe_and_create(LinuxContainmentConfig::new(
            workspace.path(),
            PathBuf::from(required("ELA_SMOKE_BWRAP_PATH")),
            PathBuf::from(required("ELA_SMOKE_WORKER_PATH")),
        ))
        .await
        .expect("M6.1 containment after restart"),
    );
    let mut tools = ToolRegistry::new();
    let contained: Arc<dyn ContainedToolPort> =
        ObservedContainedTool::new(tool, dispatches.clone());
    tools
        .register_contained(contained)
        .expect("contained tool after restart");
    let harness = Arc::new(
        base_harness(model.clone(), tools, knowledge.clone(), store.clone())
            .with_durable_local_write_approval(seal, binding),
    );
    let workflow: Arc<dyn ConfiguredWorkflow> =
        Arc::new(EnterpriseMvpWorkflow::localwrite(route).expect("workflow"));
    let service = Arc::new(AgentService::new(harness, store, vec![workflow]).expect("service"));
    service.discover_runs().await.expect("discover");
    service
        .submit_decision(key, wait.wait_id, wait.row_version, decision)
        .await
        .expect("decision");
    service
        .resume_run(key, Some(wait.wait_id))
        .await
        .expect("resume");
    let path = workspace.path().join(target);
    if decision == ApprovalDecisionV1::Approve {
        let view = wait_for(&service, key, RunDispositionV1::Completed).await;
        assert!(matches!(
            view.result,
            Some(ApplicationResultV1::LocalWriteCompleted { .. })
        ));
        assert!(path.is_file());
        assert_eq!(dispatches.load(Ordering::SeqCst), 1);
    } else {
        let view = wait_for(&service, key, RunDispositionV1::Failed).await;
        assert_eq!(view.result, Some(ApplicationResultV1::ApprovalDenied));
        assert!(!path.exists());
        assert_eq!(dispatches.load(Ordering::SeqCst), 0);
    }
    assert_eq!(model.invocations(), 1);
    assert_eq!(knowledge.invocations(), 1);
    assert!(model.last_shape().is_some());
}

#[tokio::test]
#[ignore = "requires explicit real local model, knowledge, and M6.1 environment"]
async fn real_localwrite_approved_restart_resume_contained_write() {
    real_localwrite(ApprovalDecisionV1::Approve).await;
}

#[tokio::test]
#[ignore = "requires explicit real local model, knowledge, and M6.1 environment"]
async fn real_localwrite_denied_executes_no_write() {
    real_localwrite(ApprovalDecisionV1::Deny).await;
}
