use std::{env, path::PathBuf, sync::Arc, time::Duration};

use agent_action_seal_local::LocalActionSealer;
use agent_containment_linux::{LinuxContainmentConfig, LinuxWorkspaceWriteTool};
use agent_core::{KnowledgeBackendId, WorkspaceBindingId};
use agent_harness::{
    AuditFailurePolicy, AuditSink, ExecutionHarness, HarnessConfig, M6ApprovalPolicy,
    RunPersistencePort, ToolRegistry, testing::InMemoryAuditSink,
};
use agent_knowledge::{KnowledgeBackendPort, KnowledgePort, KnowledgeRoute, RoutedKnowledgePort};
use agent_knowledge_adapters::{OcppApiConfig, OcppKnowledgeAdapter};
use agent_mvp::{EnterpriseMvpWorkflow, LOCALWRITE_WORKFLOW_ID, READONLY_WORKFLOW_ID};
use agent_persistence_sqlite::SqliteRunPersistence;
use agent_provider_rig::{
    BearerCredential, OpenAiCompatibleConfig, build_openai_compatible_model_port,
    probe_openai_compatible,
};
use agent_service::{
    AgentService, ApprovalDecisionV1, ConfiguredWorkflow, RunDispositionV1, RunInput, RunKey,
    WorkflowId,
};
use uuid::Uuid;

fn required(name: &str) -> String {
    env::var(name).unwrap_or_else(|_| panic!("{name} is required for the opted-in smoke test"))
}

fn model_config() -> OpenAiCompatibleConfig {
    let endpoint = required("ELA_SMOKE_MODEL_BASE_URL");
    let model = required("ELA_SMOKE_MODEL_ID");
    match required("ELA_SMOKE_MODEL_AUTH").as_str() {
        "no-auth-loopback" => OpenAiCompatibleConfig::new_no_auth_loopback(endpoint, model)
            .expect("explicit loopback no-auth model configuration"),
        "bearer" => OpenAiCompatibleConfig::new(
            endpoint,
            model,
            BearerCredential::new(required("ELA_SMOKE_MODEL_BEARER")).expect("bearer"),
        )
        .expect("bearer model configuration"),
        _ => panic!("ELA_SMOKE_MODEL_AUTH must be bearer or no-auth-loopback"),
    }
}

fn knowledge() -> (Arc<dyn KnowledgePort>, KnowledgeRoute) {
    let config =
        OcppApiConfig::new(url::Url::parse(&required("ELA_SMOKE_OCPP_URL")).expect("OCPP URL"))
            .expect("OCPP configuration");
    let backend: Arc<dyn KnowledgeBackendPort> =
        Arc::new(OcppKnowledgeAdapter::new(config).expect("OCPP adapter"));
    (
        Arc::new(RoutedKnowledgePort::new(vec![backend]).expect("knowledge router")),
        KnowledgeRoute::single(KnowledgeBackendId::OcppRagKag),
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

async fn wait_for(service: &Arc<AgentService>, key: RunKey, expected: RunDispositionV1) {
    for _ in 0..900 {
        if service
            .get_run_status(key)
            .await
            .is_ok_and(|view| view.disposition == expected)
        {
            return;
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
    let model = build_openai_compatible_model_port(config).expect("model");
    let (knowledge, route) = knowledge();
    let (_directory, store) = persistence().await;
    let harness = Arc::new(base_harness(
        model,
        ToolRegistry::new(),
        knowledge,
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
    wait_for(&service, key, RunDispositionV1::Completed).await;
    assert!(
        service
            .get_run_status(key)
            .await
            .expect("status")
            .result
            .is_some()
    );
}

async fn real_localwrite(decision: ApprovalDecisionV1) {
    let config = model_config();
    probe_openai_compatible(&config)
        .await
        .expect("model readiness");
    let model = build_openai_compatible_model_port(config).expect("model");
    let (knowledge, route) = knowledge();
    let (_directory, store) = persistence().await;
    let workspace = tempfile::tempdir().expect("disposable write workspace");
    let target = required("ELA_SMOKE_WRITE_RELATIVE_PATH");
    assert!(!PathBuf::from(&target).is_absolute());
    let tool = Arc::new(
        LinuxWorkspaceWriteTool::probe_and_create(LinuxContainmentConfig::new(
            workspace.path(),
            PathBuf::from(required("ELA_SMOKE_BWRAP_PATH")),
            PathBuf::from(required("ELA_SMOKE_WORKER_PATH")),
        ))
        .await
        .expect("M6.1 containment readiness"),
    );
    let mut tools = ToolRegistry::new();
    let contained: Arc<dyn agent_harness::ContainedToolPort> = tool;
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
    drop(service);

    let tool = Arc::new(
        LinuxWorkspaceWriteTool::probe_and_create(LinuxContainmentConfig::new(
            workspace.path(),
            PathBuf::from(required("ELA_SMOKE_BWRAP_PATH")),
            PathBuf::from(required("ELA_SMOKE_WORKER_PATH")),
        ))
        .await
        .expect("M6.1 containment after restart"),
    );
    let mut tools = ToolRegistry::new();
    let contained: Arc<dyn agent_harness::ContainedToolPort> = tool;
    tools
        .register_contained(contained)
        .expect("contained tool after restart");
    let harness = Arc::new(
        base_harness(model, tools, knowledge, store.clone())
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
        wait_for(&service, key, RunDispositionV1::Completed).await;
        assert!(path.is_file());
    } else {
        wait_for(&service, key, RunDispositionV1::Failed).await;
        assert!(!path.exists());
    }
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
