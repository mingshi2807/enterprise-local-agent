use std::{
    future::pending,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use agent_core::{
    KnowledgeBackendId, LoopPhase, LoopProgressEvent, ModelMessage, ModelResponse, ModelRole,
    RunBudget, RunId, SessionId,
};
use agent_harness::{
    AuditFailurePolicy, ExecutionHarness, HarnessConfig, M0ReadOnlyPolicy, ModelPort,
    RecoveryDisposition, RunContext, RunPersistencePort, ToolRegistry,
    testing::{FakeModelPort, InMemoryAuditSink},
};
use agent_knowledge::{
    BackendEvidenceSet, Evidence, EvidenceId, EvidenceMetadata, EvidenceSet, EvidenceSource,
    GroundedModelRequest, KnowledgeError, KnowledgeFuture, KnowledgePort, KnowledgeQuery,
    KnowledgeRequest, KnowledgeRoute, RetrievalLimits,
};

use crate::{
    LoopEffects, LoopEngine, LoopFuture, LoopProgram, LoopStepError, ReflectDecision,
    RestartableLoopProgram, VerificationResult,
};

const QUERY: &str = "ISO charging loop requirements";

struct CountingKnowledge {
    result: Option<EvidenceSet>,
    calls: AtomicUsize,
}

impl CountingKnowledge {
    fn pending() -> Self {
        Self {
            result: None,
            calls: AtomicUsize::new(0),
        }
    }

    fn succeeding(result: EvidenceSet) -> Self {
        Self {
            result: Some(result),
            calls: AtomicUsize::new(0),
        }
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

impl KnowledgePort for CountingKnowledge {
    fn retrieve<'a>(
        &'a self,
        _request: KnowledgeRequest,
    ) -> KnowledgeFuture<'a, Result<EvidenceSet, KnowledgeError>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        match &self.result {
            Some(result) => Box::pin(std::future::ready(Ok(result.clone()))),
            None => Box::pin(pending()),
        }
    }
}

struct KnowledgeProgram;

struct KnowledgeState {
    retrieval: Option<agent_harness::CompletedKnowledgeRetrieval>,
}

impl LoopProgram for KnowledgeProgram {
    type WorkingState = KnowledgeState;

    fn observe<'a>(
        &'a mut self,
        _iteration: u32,
        _state: &'a mut Self::WorkingState,
        _effects: LoopEffects<'a>,
    ) -> LoopFuture<'a, Result<(), LoopStepError>> {
        Box::pin(std::future::ready(Ok(())))
    }

    fn retrieve<'a>(
        &'a mut self,
        _iteration: u32,
        state: &'a mut Self::WorkingState,
        mut effects: LoopEffects<'a>,
    ) -> LoopFuture<'a, Result<(), LoopStepError>> {
        Box::pin(async move {
            state.retrieval = Some(effects.retrieve_knowledge(request()).await?);
            Ok(())
        })
    }

    fn plan<'a>(
        &'a mut self,
        _iteration: u32,
        state: &'a mut Self::WorkingState,
        mut effects: LoopEffects<'a>,
    ) -> LoopFuture<'a, Result<(), LoopStepError>> {
        Box::pin(async move {
            let retrieval = state.retrieval.as_ref().ok_or(LoopStepError::Program)?;
            let grounded = GroundedModelRequest::new(
                vec![ModelMessage::new(ModelRole::User, "Answer with citations")],
                retrieval.evidence(),
            );
            effects.invoke_grounded_model(retrieval, grounded).await?;
            Ok(())
        })
    }

    fn act<'a>(
        &'a mut self,
        _iteration: u32,
        _state: &'a mut Self::WorkingState,
        _effects: LoopEffects<'a>,
    ) -> LoopFuture<'a, Result<(), LoopStepError>> {
        Box::pin(std::future::ready(Ok(())))
    }

    fn verify<'a>(
        &'a mut self,
        _iteration: u32,
        _state: &'a mut Self::WorkingState,
        _effects: LoopEffects<'a>,
    ) -> LoopFuture<'a, Result<VerificationResult, LoopStepError>> {
        Box::pin(std::future::ready(Ok(VerificationResult::Passed)))
    }

    fn reflect<'a>(
        &'a mut self,
        _iteration: u32,
        _state: &'a mut Self::WorkingState,
        _verification: VerificationResult,
        _effects: LoopEffects<'a>,
    ) -> LoopFuture<'a, Result<ReflectDecision, LoopStepError>> {
        Box::pin(std::future::ready(Ok(ReflectDecision::Complete)))
    }
}

impl RestartableLoopProgram for KnowledgeProgram {
    const RECOVERY_VERSION: u32 = 8;
    const RESTART_INTERRUPTED_RETRIEVAL: bool = true;

    fn restore_working_state(
        &mut self,
        _state: &agent_harness::DurableRunState,
    ) -> Result<Self::WorkingState, LoopStepError> {
        Ok(KnowledgeState { retrieval: None })
    }
}

fn request() -> KnowledgeRequest {
    KnowledgeRequest::new(
        KnowledgeQuery::new(QUERY).expect("query"),
        KnowledgeRoute::single(KnowledgeBackendId::StandardsMcp),
        RetrievalLimits::default(),
    )
}

fn evidence_set() -> EvidenceSet {
    let backend = KnowledgeBackendId::StandardsMcp;
    let evidence = Evidence::new(
        EvidenceId::new("standards:chunk-1").expect("id"),
        EvidenceSource::new(backend, "ISO15118-20").expect("source"),
        "The charging loop has bounded protocol semantics.".to_owned(),
        1,
        None,
        Some("ISO15118-20".to_owned()),
        Some("chunk-1".to_owned()),
        "ISO15118-20:chunk-1".to_owned(),
        None,
        None,
        EvidenceMetadata::Standards {
            source_title: "ISO 15118-20".to_owned(),
            section: Some("8.4".to_owned()),
            heading: Some("Charge loop".to_owned()),
            page_range: Some("42-43".to_owned()),
            chunk_type: "text".to_owned(),
        },
    )
    .expect("evidence");
    EvidenceSet::from_backend(
        BackendEvidenceSet::new(backend, vec![evidence], None, false).expect("backend set"),
        RetrievalLimits::default(),
    )
    .expect("set")
}

fn harness(
    model: Arc<FakeModelPort>,
    knowledge: Arc<dyn KnowledgePort>,
    persistence: Arc<dyn RunPersistencePort>,
) -> ExecutionHarness {
    let model: Arc<dyn ModelPort> = model;
    ExecutionHarness::new(
        model,
        ToolRegistry::new(),
        Arc::new(M0ReadOnlyPolicy),
        Arc::new(InMemoryAuditSink::new()),
        HarnessConfig::new(AuditFailurePolicy::FailClosed, Duration::from_secs(1)).expect("config"),
    )
    .with_knowledge_port(knowledge)
    .with_persistence_port(persistence)
}

async fn enter_retrieve(harness: &ExecutionHarness, context: &mut RunContext) {
    harness.start_run(context).await.expect("start");
    let iteration = harness.begin_iteration(context).await.expect("iteration");
    for event in [
        LoopProgressEvent::PhaseEntered {
            iteration,
            phase: LoopPhase::Observe,
        },
        LoopProgressEvent::PhaseCompleted {
            iteration,
            phase: LoopPhase::Observe,
        },
        LoopProgressEvent::PhaseEntered {
            iteration,
            phase: LoopPhase::Retrieve,
        },
    ] {
        harness
            .record_loop_progress(context, event)
            .await
            .expect("progress");
    }
}

#[tokio::test]
async fn interrupted_retrieval_replay_is_inert_then_explicit_resume_retrieves_fresh() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let store = Arc::new(
        agent_persistence_sqlite::SqliteRunPersistence::open(directory.path().join("runs.sqlite3"))
            .await
            .expect("store"),
    );
    let pending_knowledge = Arc::new(CountingKnowledge::pending());
    let first_model = Arc::new(FakeModelPort::scripted(Vec::new()));
    let first_harness = Arc::new(harness(
        first_model.clone(),
        pending_knowledge.clone(),
        store.clone(),
    ));
    let run_id = RunId::new();
    let session_id = SessionId::new();
    let mut context = RunContext::new_restartable_retrieval(
        run_id,
        session_id,
        RunBudget::new(1, 0, 1, Duration::from_secs(30)).expect("budget"),
        KnowledgeProgram::RECOVERY_VERSION,
    );
    enter_retrieve(&first_harness, &mut context).await;

    let task_harness = first_harness.clone();
    let task = tokio::spawn(async move {
        let _ = task_harness
            .retrieve_knowledge(&mut context, request())
            .await;
    });
    tokio::time::timeout(Duration::from_secs(1), async {
        while pending_knowledge.calls() == 0 {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .expect("retrieval must reach the backend");
    assert_eq!(pending_knowledge.calls(), 1);
    task.abort();
    let _ = task.await;

    let resumed_knowledge = Arc::new(CountingKnowledge::succeeding(evidence_set()));
    let resumed_model = Arc::new(FakeModelPort::scripted(vec![Ok(ModelResponse::new(
        Vec::new(),
        None,
    ))]));
    let resumed_harness = harness(resumed_model.clone(), resumed_knowledge.clone(), store);
    let disposition = resumed_harness
        .recover_run(agent_harness::RunKey::new(run_id, session_id))
        .await
        .expect("recover");
    assert_eq!(resumed_knowledge.calls(), 0, "replay must not retrieve");
    assert_eq!(resumed_model.invocation_count(), 0, "replay must not plan");
    let RecoveryDisposition::Resumable(recovered) = disposition else {
        panic!("retrieval-aware run must be resumable");
    };

    let mut program = KnowledgeProgram;
    let (summary, state) = LoopEngine::new()
        .resume(&resumed_harness, *recovered, &mut program)
        .await
        .expect("resume");

    assert_eq!(resumed_knowledge.calls(), 1);
    assert_eq!(resumed_model.invocation_count(), 1);
    assert!(state.retrieval.is_some());
    assert_eq!(summary.completed_iterations(), 1);
    assert_eq!(
        summary.terminal_decision(),
        crate::TerminalLoopDecision::Complete
    );
}

#[tokio::test]
async fn full_retrieve_evidence_grounded_plan_flow_is_harness_governed() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let store = Arc::new(
        agent_persistence_sqlite::SqliteRunPersistence::open(directory.path().join("runs.sqlite3"))
            .await
            .expect("store"),
    );
    let knowledge = Arc::new(CountingKnowledge::succeeding(evidence_set()));
    let model = Arc::new(FakeModelPort::scripted(vec![Ok(ModelResponse::new(
        Vec::new(),
        None,
    ))]));
    let harness = harness(model.clone(), knowledge.clone(), store);
    let mut context = RunContext::new_restartable_retrieval(
        RunId::new(),
        SessionId::new(),
        RunBudget::new(1, 0, 1, Duration::from_secs(30)).expect("budget"),
        KnowledgeProgram::RECOVERY_VERSION,
    );
    harness.start_run(&mut context).await.expect("start");
    let mut program = KnowledgeProgram;
    let mut state = KnowledgeState { retrieval: None };

    let summary = LoopEngine::new()
        .run(&harness, &mut context, &mut program, &mut state)
        .await
        .expect("run");

    assert_eq!(knowledge.calls(), 1);
    assert_eq!(model.invocation_count(), 1);
    assert_eq!(context.usage().tool_calls(), 0);
    assert!(state.retrieval.is_some());
    assert_eq!(summary.completed_iterations(), 1);
}
