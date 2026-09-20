use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use agent_action_seal_local::LocalActionSealer;
use agent_core::{
    CapabilityKind, KnowledgeBackendId, ModelOutputPart, ModelResponse, ToolDefinition, ToolName,
    ToolOutput, ToolSchema, WorkspaceBindingId,
};
use agent_harness::{
    AuditFailurePolicy, AuditSink, ExecutionHarness, HarnessConfig, M6ApprovalPolicy, ModelPort,
    ModelPortError, RunPersistencePort, ToolRegistry,
    testing::{FakeContainedToolPort, FakeModelPort, InMemoryAuditSink},
};
use agent_identity::{
    ApprovalSeparation, PrincipalRole, VerifiedPrincipal,
    testing::{InMemorySecurityAudit, policy, principal},
};
use agent_knowledge::{
    BackendEvidenceSet, Evidence, EvidenceMetadata, EvidenceSet, EvidenceSource, KnowledgeError,
    KnowledgeFuture, KnowledgePort,
};
use agent_persistence_sqlite::SqliteRunPersistence;
use agent_service::{AgentService, ApprovalDecisionV1, ConfiguredWorkflow, RunDispositionV1};
use serde_json::json;
use uuid::Uuid;

use super::*;

fn test_principal() -> VerifiedPrincipal {
    principal(
        "mvp-test-user",
        &[PrincipalRole::User, PrincipalRole::Approver],
    )
}

fn requester_principal() -> VerifiedPrincipal {
    principal("mvp-requester", &[PrincipalRole::User])
}

fn approver_principal() -> VerifiedPrincipal {
    principal("mvp-approver", &[PrincipalRole::Approver])
}

fn build_service(
    harness: Arc<ExecutionHarness>,
    persistence: Arc<SqliteRunPersistence>,
    workflows: Vec<Arc<dyn ConfiguredWorkflow>>,
) -> AgentService {
    let workflow_ids = workflows
        .iter()
        .map(|workflow| workflow.id().as_str().to_owned())
        .collect::<Vec<_>>();
    AgentService::new(
        harness,
        persistence.clone(),
        persistence,
        Arc::new(policy(
            workflow_ids,
            ApprovalSeparation::RequesterMayApprove,
        )),
        Arc::new(InMemorySecurityAudit::default()),
        workflows,
    )
    .expect("service")
}

struct FixedKnowledge {
    evidence: EvidenceSet,
    calls: AtomicUsize,
}

impl FixedKnowledge {
    fn new() -> Self {
        let evidence = Evidence::new(
            EvidenceId::new("ocpp:chunk-1").expect("evidence id"),
            EvidenceSource::new(KnowledgeBackendId::OcppRagKag, "OCPP 2.0.1").expect("source"),
            "Trusted backend content treated as untrusted model data.".to_owned(),
            1,
            None,
            Some("doc-1".to_owned()),
            Some("chunk-1".to_owned()),
            "doc-1:chunk-1".to_owned(),
            Some("OCPP-2.0.1/K01/3.1".to_owned()),
            Some("snapshot-1".to_owned()),
            EvidenceMetadata::Ocpp {
                strategy: "hybrid".to_owned(),
                section_title: Some("ChargingProfile".to_owned()),
                page_start: Some(10),
                page_end: Some(11),
                evidence_layer: Some("spec".to_owned()),
                source_type: Some("spec_pdf".to_owned()),
            },
        )
        .expect("evidence");
        let backend =
            BackendEvidenceSet::new(KnowledgeBackendId::OcppRagKag, vec![evidence], None, false)
                .expect("backend set");
        Self {
            evidence: EvidenceSet::from_backend(backend, RetrievalLimits::default())
                .expect("evidence set"),
            calls: AtomicUsize::new(0),
        }
    }
}

impl KnowledgePort for FixedKnowledge {
    fn retrieve<'a>(
        &'a self,
        _request: KnowledgeRequest,
    ) -> KnowledgeFuture<'a, Result<EvidenceSet, KnowledgeError>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Box::pin(std::future::ready(Ok(self.evidence.clone())))
    }
}

struct FailingKnowledge;

impl KnowledgePort for FailingKnowledge {
    fn retrieve<'a>(
        &'a self,
        request: KnowledgeRequest,
    ) -> KnowledgeFuture<'a, Result<EvidenceSet, KnowledgeError>> {
        Box::pin(std::future::ready(Err(KnowledgeError::Unavailable(
            request.route().backends()[0],
        ))))
    }
}

fn model(text: &str) -> Arc<FakeModelPort> {
    Arc::new(FakeModelPort::scripted(vec![Ok(ModelResponse::new(
        vec![ModelOutputPart::Text(text.to_owned())],
        None,
    ))]))
}

async fn store() -> (tempfile::TempDir, Arc<SqliteRunPersistence>) {
    let directory = tempfile::tempdir().expect("directory");
    let store = Arc::new(
        SqliteRunPersistence::open(directory.path().join("runs.sqlite3"))
            .await
            .expect("store"),
    );
    (directory, store)
}

fn build_harness(
    model: Arc<dyn ModelPort>,
    tools: ToolRegistry,
    knowledge: Arc<dyn KnowledgePort>,
    persistence: Arc<dyn RunPersistencePort>,
) -> ExecutionHarness {
    ExecutionHarness::new(
        model,
        tools,
        Arc::new(M6ApprovalPolicy),
        Arc::new(InMemoryAuditSink::new()) as Arc<dyn AuditSink>,
        HarnessConfig::new(AuditFailurePolicy::FailClosed, Duration::from_secs(2)).expect("config"),
    )
    .with_persistence_port(persistence)
    .with_knowledge_port(knowledge)
}

fn write_definition() -> ToolDefinition {
    ToolDefinition::new(
        ToolName::new("workspace_write_file").expect("tool name"),
        "write one bounded UTF-8 file beneath the configured workspace",
        CapabilityKind::LocalWrite,
        ToolSchema::new(json!({
            "type":"object",
            "properties": {
                "relative_path":{"type":"string","maxLength":240},
                "content":{"type":"string","maxLength":4096}
            },
            "required":["relative_path","content"],
            "additionalProperties":false
        }))
        .expect("schema"),
    )
    .expect("definition")
}

async fn wait_for(
    service: &Arc<AgentService>,
    principal: &VerifiedPrincipal,
    key: RunKey,
    expected: RunDispositionV1,
) -> agent_service::RunView {
    for _ in 0..200 {
        if let Ok(view) = service.get_run_status(principal, key).await
            && view.disposition == expected
        {
            return view;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("run did not reach {expected:?}");
}

#[test]
fn reviewed_graphs_are_valid_and_distinct() {
    let route = KnowledgeRoute::single(KnowledgeBackendId::OcppRagKag);
    let readonly = EnterpriseMvpWorkflow::readonly(route.clone()).expect("readonly graph");
    let localwrite = EnterpriseMvpWorkflow::localwrite(route).expect("localwrite graph");
    assert_eq!(readonly.id().as_str(), READONLY_WORKFLOW_ID);
    assert_eq!(localwrite.id().as_str(), LOCALWRITE_WORKFLOW_ID);
    assert_ne!(readonly.graph().digest(), localwrite.graph().digest());
}

#[test]
fn strict_output_union_rejects_mixed_extra_duplicate_and_prose() {
    assert!(matches!(
        decode_model_text(r#"{"final_answer":"ok","citations":[]}"#),
        Ok(DecodedModelOutput::FinalAnswer(_))
    ));
    assert!(matches!(
        decode_model_text(r#"{"action":{"tool":"workspace_write_file","arguments":{}}}"#),
        Ok(DecodedModelOutput::Action)
    ));
    for invalid in [
        r#"{"final_answer":"ok","citations":[],"action":{}}"#,
        r#"{"final_answer":"ok","citations":[],"extra":1}"#,
        r#"{"final_answer":"a","final_answer":"b","citations":[]}"#,
        "```json\n{}\n```",
        "not json",
        r#"{"final_answer":"ok"}"#,
    ] {
        assert!(decode_model_text(invalid).is_err(), "accepted {invalid}");
    }
}

#[test]
fn final_answer_bounds_are_enforced() {
    let oversized = format!(
        r#"{{"final_answer":"{}","citations":[]}}"#,
        "x".repeat(agent_service::MAX_FINAL_ANSWER_BYTES + 1)
    );
    assert!(decode_model_text(&oversized).is_err());
    let too_many = json!({
        "final_answer": "ok",
        "citations": vec!["id"; agent_service::MAX_RESULT_CITATIONS + 1]
    });
    assert!(decode_model_text(&too_many.to_string()).is_err());
}

#[tokio::test]
async fn readonly_workflow_completes_without_containment_and_binds_citations() {
    let (_directory, persistence) = store().await;
    let knowledge = Arc::new(FixedKnowledge::new());
    let fake_model =
        model(r#"{"final_answer":"Use the cited profile rules.","citations":["ocpp:chunk-1"]}"#);
    let harness = Arc::new(build_harness(
        fake_model.clone(),
        ToolRegistry::new(),
        knowledge.clone(),
        persistence.clone(),
    ));
    let workflow: Arc<dyn ConfiguredWorkflow> = Arc::new(
        EnterpriseMvpWorkflow::readonly(KnowledgeRoute::single(KnowledgeBackendId::OcppRagKag))
            .expect("workflow"),
    );
    let service = Arc::new(build_service(harness, persistence, vec![workflow]));
    let principal = test_principal();
    let session = service.create_session(&principal).await.expect("session");
    let view = service
        .start_run(
            &principal,
            session,
            Uuid::new_v4(),
            &WorkflowId::new(READONLY_WORKFLOW_ID).expect("id"),
            RunInput::new(b"How should a charging profile be applied?".to_vec()).expect("input"),
        )
        .await
        .expect("start");
    let completed = wait_for(
        &service,
        &principal,
        RunKey::new(view.run_id, session),
        RunDispositionV1::Completed,
    )
    .await;
    let ApplicationResultV1::FinalAnswer { answer, citations } = completed.result.expect("result")
    else {
        panic!("final result")
    };
    assert_eq!(answer, "Use the cited profile rules.");
    assert_eq!(citations.len(), 1);
    assert_eq!(citations[0].reference_id, "doc-1:chunk-1");
    assert_eq!(fake_model.invocation_count(), 1);
    assert_eq!(knowledge.calls.load(Ordering::SeqCst), 1);
    let events = service
        .read_events(&principal, RunKey::new(view.run_id, session), None, 64)
        .await
        .expect("events");
    let serialized = serde_json::to_string(&events).expect("event JSON");
    for forbidden in [
        "Use the cited profile rules.",
        "Trusted backend content",
        "How should a charging profile be applied?",
    ] {
        assert!(!serialized.contains(forbidden));
    }
    assert!(events.iter().all(|event| event.version == 2));
    assert!(
        events
            .iter()
            .any(|event| !event.knowledge_backends.is_empty())
    );
}

#[tokio::test]
async fn model_knowledge_and_citation_failures_terminalize_without_results() {
    let cases: Vec<(Arc<dyn ModelPort>, Arc<dyn KnowledgePort>)> = vec![
        (
            Arc::new(FakeModelPort::scripted(vec![Err(
                ModelPortError::Unavailable,
            )])),
            Arc::new(FixedKnowledge::new()),
        ),
        (
            model(r#"{"final_answer":"answer","citations":[]}"#),
            Arc::new(FailingKnowledge),
        ),
        (
            model(r#"{"final_answer":"answer","citations":["unknown"]}"#),
            Arc::new(FixedKnowledge::new()),
        ),
    ];
    for (model, knowledge) in cases {
        let (_directory, persistence) = store().await;
        let service = Arc::new(build_service(
            Arc::new(build_harness(
                model,
                ToolRegistry::new(),
                knowledge,
                persistence.clone(),
            )),
            persistence,
            vec![Arc::new(
                EnterpriseMvpWorkflow::readonly(KnowledgeRoute::single(
                    KnowledgeBackendId::OcppRagKag,
                ))
                .expect("workflow"),
            )],
        ));
        let principal = test_principal();
        let session = service.create_session(&principal).await.expect("session");
        let run = service
            .start_run(
                &principal,
                session,
                Uuid::new_v4(),
                &WorkflowId::new(READONLY_WORKFLOW_ID).expect("id"),
                RunInput::new(b"bounded question".to_vec()).expect("input"),
            )
            .await
            .expect("start");
        let terminal = wait_for(
            &service,
            &principal,
            RunKey::new(run.run_id, session),
            RunDispositionV1::Failed,
        )
        .await;
        assert!(terminal.result.is_none());
    }
}

#[tokio::test]
async fn oversized_mvp_input_is_rejected_before_run_or_model_invocation() {
    let (_directory, persistence) = store().await;
    let knowledge = Arc::new(FixedKnowledge::new());
    let fake_model = model(r#"{"final_answer":"unused","citations":[]}"#);
    let service = Arc::new(build_service(
        Arc::new(build_harness(
            fake_model.clone(),
            ToolRegistry::new(),
            knowledge.clone(),
            persistence.clone(),
        )),
        persistence,
        vec![Arc::new(
            EnterpriseMvpWorkflow::readonly(KnowledgeRoute::single(KnowledgeBackendId::OcppRagKag))
                .expect("workflow"),
        )],
    ));
    let requester = requester_principal();
    let session = service.create_session(&requester).await.expect("session");
    let result = service
        .start_run(
            &requester,
            session,
            Uuid::new_v4(),
            &WorkflowId::new(READONLY_WORKFLOW_ID).expect("id"),
            RunInput::new(vec![b'x'; agent_knowledge::MAX_QUERY_BYTES + 1]).expect("service input"),
        )
        .await;
    assert!(matches!(
        result,
        Err(agent_service::ServiceError::InvalidRequest)
    ));
    assert_eq!(fake_model.invocation_count(), 0);
    assert_eq!(knowledge.calls.load(Ordering::SeqCst), 0);
}

async fn localwrite_fixture(
    decision: ApprovalDecisionV1,
) -> (usize, RunDispositionV1, Option<ApplicationResultV1>) {
    let (_directory, persistence) = store().await;
    let knowledge = Arc::new(FixedKnowledge::new());
    let fake_model = model(
        r#"{"action":{"tool":"workspace_write_file","arguments":{"relative_path":"report.txt","content":"bounded content"}}}"#,
    );
    let tool = Arc::new(FakeContainedToolPort::succeeding(
        write_definition(),
        ToolOutput::new(json!({"written":true})),
    ));
    let mut registry = ToolRegistry::new();
    let contained: Arc<dyn agent_harness::ContainedToolPort> = tool.clone();
    registry.register_contained(contained).expect("register");
    let seal = Arc::new(LocalActionSealer::new("m13-test", [0x31; 32]).expect("seal"));
    let binding = WorkspaceBindingId::new();
    let harness = Arc::new(
        build_harness(
            fake_model.clone(),
            registry,
            knowledge.clone(),
            persistence.clone(),
        )
        .with_durable_local_write_approval(seal.clone(), binding),
    );
    let workflow: Arc<dyn ConfiguredWorkflow> = Arc::new(
        EnterpriseMvpWorkflow::localwrite(KnowledgeRoute::single(KnowledgeBackendId::OcppRagKag))
            .expect("workflow"),
    );
    let service = Arc::new(build_service(harness, persistence.clone(), vec![workflow]));
    let requester = requester_principal();
    let approver = approver_principal();
    let session = service.create_session(&requester).await.expect("session");
    let start = service
        .start_run(
            &requester,
            session,
            Uuid::new_v4(),
            &WorkflowId::new(LOCALWRITE_WORKFLOW_ID).expect("id"),
            RunInput::new(b"Create a bounded report.".to_vec()).expect("input"),
        )
        .await
        .expect("start");
    let key = RunKey::new(start.run_id, session);
    wait_for(&service, &requester, key, RunDispositionV1::Waiting).await;
    let (waiting, _) = service
        .waiting_page(&approver, None, 8)
        .await
        .expect("waiting page");
    let wait = waiting.first().expect("wait").clone();

    drop(service);
    let mut registry = ToolRegistry::new();
    let contained: Arc<dyn agent_harness::ContainedToolPort> = tool.clone();
    registry
        .register_contained(contained)
        .expect("register after restart");
    let restarted_harness = Arc::new(
        build_harness(fake_model, registry, knowledge, persistence.clone())
            .with_durable_local_write_approval(seal, binding),
    );
    let workflow: Arc<dyn ConfiguredWorkflow> = Arc::new(
        EnterpriseMvpWorkflow::localwrite(KnowledgeRoute::single(KnowledgeBackendId::OcppRagKag))
            .expect("workflow"),
    );
    let restarted = Arc::new(build_service(
        restarted_harness,
        persistence.clone(),
        vec![workflow],
    ));
    restarted.discover_runs().await.expect("discover");
    restarted
        .submit_decision(&approver, key, wait.wait_id, wait.row_version, decision)
        .await
        .expect("decision");
    let persisted = persistence
        .load_durable_approval_wait(key, wait.wait_id)
        .await
        .expect("persisted decision");
    assert_eq!(persisted.decision_actor(), Some(approver.id()));
    assert!(persisted.decision_timestamp_unix_millis().is_some());
    restarted
        .resume_run(&requester, key, Some(wait.wait_id))
        .await
        .expect("resume");
    let expected = if decision == ApprovalDecisionV1::Approve {
        RunDispositionV1::Completed
    } else {
        RunDispositionV1::Failed
    };
    let terminal = wait_for(&restarted, &requester, key, expected).await;
    (
        tool.invocation_count(),
        terminal.disposition,
        terminal.result,
    )
}

#[tokio::test]
async fn approved_localwrite_survives_restart_and_executes_once() {
    let (calls, disposition, result) = localwrite_fixture(ApprovalDecisionV1::Approve).await;
    assert_eq!(calls, 1);
    assert_eq!(disposition, RunDispositionV1::Completed);
    assert!(matches!(
        result,
        Some(ApplicationResultV1::LocalWriteCompleted { .. })
    ));
}

#[tokio::test]
async fn denied_localwrite_survives_restart_and_dispatches_zero_tools() {
    let (calls, disposition, result) = localwrite_fixture(ApprovalDecisionV1::Deny).await;
    assert_eq!(calls, 0);
    assert_eq!(disposition, RunDispositionV1::Failed);
    assert_eq!(result, Some(ApplicationResultV1::ApprovalDenied));
}
