use crate::{
    ActionEffects, DecisionContext, Edge, GraphBranchId, GraphDefinition, GraphDefinitionError,
    GraphFuture, GraphNodeId, GraphProgram, GraphProgramError, GraphRecoveryMode,
    GraphTerminalOutcome, GraphTransitionKey, MAX_GRAPH_NODES, ModelEffects, NodeDefinition,
    NodeKind, RestartableGraphProgram, RetrieveEffects, VerificationOutcome, VerifyEffects,
};
use agent_core::{
    AgentEventKind, BudgetDimension, CapabilityKind, KnowledgeBackendId, ModelMessage,
    ModelOutputPart, ModelResponse, ModelRole, RunBudget, RunId, RunOutcome, RunStatus, SessionId,
    ToolDefinition, ToolName, ToolOutput, ToolResult, ToolSchema,
};
use agent_harness::{
    AppendTransition, AuditFailurePolicy, CompletedKnowledgeRetrieval, CompletedModelInvocation,
    ContainedToolPort, DurableCheckpoint, ExecutionHarness, HarnessConfig, LoadedRun,
    M0ReadOnlyPolicy, M6ApprovalPolicy, ModelPort, PersistenceFuture, PersistencePortError,
    RecoveryDisposition, RunContext, RunKey, RunPersistencePort, RunRecord, ToolPort, ToolRegistry,
    testing::{
        FakeContainedToolPort, FakeModelPort, FakeToolPort, InMemoryAuditSink, ScriptedApprovalPort,
    },
};
use agent_knowledge::{
    BackendEvidenceSet, Evidence, EvidenceId, EvidenceMetadata, EvidenceSet, EvidenceSource,
    GroundedModelRequest, KnowledgeError, KnowledgeFuture, KnowledgePort, KnowledgeQuery,
    KnowledgeRequest, KnowledgeRoute, RetrievalLimits,
};
use std::{
    future::pending,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

fn id(value: &str) -> GraphNodeId {
    GraphNodeId::new(value).unwrap_or_else(|error| panic!("invalid test node ID: {error}"))
}

fn node(value: &str, kind: NodeKind) -> NodeDefinition {
    let recovery = match kind {
        NodeKind::Retrieve => GraphRecoveryMode::FreshRetrieval,
        NodeKind::Decision { .. } => GraphRecoveryMode::DeterministicBoundary,
        _ => GraphRecoveryMode::Never,
    };
    NodeDefinition::new(id(value), kind, recovery)
}

fn edge(from: &str, transition: GraphTransitionKey, to: &str) -> Edge {
    Edge::new(id(from), transition, id(to))
}

fn linear_definition(reverse: bool) -> GraphDefinition {
    let mut nodes = vec![
        node("retrieve", NodeKind::Retrieve),
        node("model", NodeKind::Model),
        node("action", NodeKind::Action),
        node("verify", NodeKind::Verify),
        node("complete", NodeKind::Complete),
        node("fail", NodeKind::Fail),
    ];
    let mut edges = vec![
        edge("retrieve", GraphTransitionKey::Succeeded, "model"),
        edge("model", GraphTransitionKey::Succeeded, "action"),
        edge("action", GraphTransitionKey::Succeeded, "verify"),
        edge("verify", GraphTransitionKey::VerificationPassed, "complete"),
        edge("verify", GraphTransitionKey::VerificationFailed, "fail"),
    ];
    if reverse {
        nodes.reverse();
        edges.reverse();
    }
    GraphDefinition::new(id("retrieve"), nodes, edges)
        .unwrap_or_else(|error| panic!("valid test graph rejected: {error}"))
}

#[test]
fn validates_the_poc_graph_and_digest_is_order_independent() {
    let first = linear_definition(false);
    let second = linear_definition(true);

    assert_eq!(first.digest(), second.digest());
    assert_eq!(first.start(), &id("retrieve"));
}

#[test]
fn rejects_cycles() {
    let result = GraphDefinition::new(
        id("a"),
        vec![node("a", NodeKind::Model), node("b", NodeKind::Action)],
        vec![
            edge("a", GraphTransitionKey::Succeeded, "b"),
            edge("b", GraphTransitionKey::Succeeded, "a"),
        ],
    );

    assert!(matches!(result, Err(GraphDefinitionError::Cycle)));
}

#[test]
fn rejects_unreachable_nodes() {
    let result = GraphDefinition::new(
        id("retrieve"),
        vec![
            node("retrieve", NodeKind::Retrieve),
            node("complete", NodeKind::Complete),
            node("orphan", NodeKind::Fail),
        ],
        vec![edge("retrieve", GraphTransitionKey::Succeeded, "complete")],
    );

    assert!(matches!(result, Err(GraphDefinitionError::UnreachableNode)));
}

#[test]
fn rejects_missing_and_wrongly_typed_transitions() {
    let missing = GraphDefinition::new(
        id("retrieve"),
        vec![
            node("retrieve", NodeKind::Retrieve),
            node("complete", NodeKind::Complete),
        ],
        Vec::new(),
    );
    assert!(matches!(
        missing,
        Err(GraphDefinitionError::InvalidTransition)
    ));

    let wrong = GraphDefinition::new(
        id("retrieve"),
        vec![
            node("retrieve", NodeKind::Retrieve),
            node("complete", NodeKind::Complete),
        ],
        vec![edge(
            "retrieve",
            GraphTransitionKey::VerificationPassed,
            "complete",
        )],
    );
    assert!(matches!(
        wrong,
        Err(GraphDefinitionError::InvalidTransition)
    ));
}

#[test]
fn rejects_terminal_outgoing_edges() {
    let result = GraphDefinition::new(
        id("complete"),
        vec![node("complete", NodeKind::Complete)],
        vec![edge("complete", GraphTransitionKey::Succeeded, "complete")],
    );

    assert!(matches!(
        result,
        Err(GraphDefinitionError::TerminalOutgoingEdge)
    ));
}

#[test]
fn rejects_recovery_modes_that_overstate_restartability() {
    let result = GraphDefinition::new(
        id("model"),
        vec![
            NodeDefinition::new(
                id("model"),
                NodeKind::Model,
                GraphRecoveryMode::FreshRetrieval,
            ),
            node("complete", NodeKind::Complete),
        ],
        vec![edge("model", GraphTransitionKey::Succeeded, "complete")],
    );

    assert!(matches!(
        result,
        Err(GraphDefinitionError::InvalidRecoveryMode)
    ));
}

#[test]
fn decision_transitions_are_typed_and_deterministic() {
    let passed = GraphBranchId::new("passed").expect("branch");
    let failed = GraphBranchId::new("failed").expect("branch");
    let definition = GraphDefinition::new(
        id("decision"),
        vec![
            node(
                "decision",
                NodeKind::Decision {
                    branches: vec![failed.clone(), passed.clone()],
                },
            ),
            node("complete", NodeKind::Complete),
            node("fail", NodeKind::Fail),
        ],
        vec![
            edge(
                "decision",
                GraphTransitionKey::Branch(passed.clone()),
                "complete",
            ),
            edge(
                "decision",
                GraphTransitionKey::Branch(failed.clone()),
                "fail",
            ),
        ],
    )
    .expect("definition");

    assert_eq!(
        definition.transition(&id("decision"), &GraphTransitionKey::Branch(passed)),
        Some(&id("complete"))
    );
    assert_eq!(
        definition.transition(&id("decision"), &GraphTransitionKey::Branch(failed)),
        Some(&id("fail"))
    );
}

#[test]
fn rejects_graphs_above_the_hard_node_ceiling() {
    let nodes = (0..=MAX_GRAPH_NODES)
        .map(|index| node(&format!("node-{index}"), NodeKind::Complete))
        .collect();
    assert!(matches!(
        GraphDefinition::new(id("node-0"), nodes, Vec::new()),
        Err(GraphDefinitionError::SizeLimit)
    ));
}

struct CountingKnowledge {
    evidence: EvidenceSet,
    calls: AtomicUsize,
}

impl CountingKnowledge {
    fn new(evidence: EvidenceSet) -> Self {
        Self {
            evidence,
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
        Box::pin(std::future::ready(Ok(self.evidence.clone())))
    }
}

#[derive(Default)]
struct PocState {
    retrieval: Option<CompletedKnowledgeRetrieval>,
    model: Option<CompletedModelInvocation>,
    result: Option<ToolResult>,
}

struct PocProgram {
    callbacks: Arc<AtomicUsize>,
}

impl GraphProgram for PocProgram {
    type WorkingState = PocState;

    fn retrieve<'a>(
        &'a mut self,
        _node_id: &'a GraphNodeId,
        state: &'a mut Self::WorkingState,
        mut effects: RetrieveEffects<'a>,
    ) -> GraphFuture<'a, Result<(), GraphProgramError>> {
        self.callbacks.fetch_add(1, Ordering::SeqCst);
        Box::pin(async move {
            state.retrieval = Some(
                effects
                    .retrieve_knowledge(knowledge_request())
                    .await
                    .map_err(|_| GraphProgramError::Failed)?,
            );
            Ok(())
        })
    }

    fn model<'a>(
        &'a mut self,
        _node_id: &'a GraphNodeId,
        state: &'a mut Self::WorkingState,
        mut effects: ModelEffects<'a>,
    ) -> GraphFuture<'a, Result<(), GraphProgramError>> {
        self.callbacks.fetch_add(1, Ordering::SeqCst);
        Box::pin(async move {
            let retrieval = state.retrieval.as_ref().ok_or(GraphProgramError::Failed)?;
            let request = GroundedModelRequest::new(
                vec![ModelMessage::new(
                    ModelRole::User,
                    "prepare the approved action",
                )],
                retrieval.evidence(),
            );
            state.model = Some(
                effects
                    .invoke_grounded_model(retrieval, request)
                    .await
                    .map_err(|_| GraphProgramError::Failed)?,
            );
            Ok(())
        })
    }

    fn action<'a>(
        &'a mut self,
        _node_id: &'a GraphNodeId,
        state: &'a mut Self::WorkingState,
        mut effects: ActionEffects<'a>,
    ) -> GraphFuture<'a, Result<(), GraphProgramError>> {
        self.callbacks.fetch_add(1, Ordering::SeqCst);
        Box::pin(async move {
            let invocation = state.model.take().ok_or(GraphProgramError::Failed)?;
            let action = effects
                .prepare_action(invocation)
                .await
                .map_err(|_| GraphProgramError::Failed)?;
            state.result = Some(
                effects
                    .invoke_validated_action(action)
                    .await
                    .map_err(|_| GraphProgramError::Failed)?,
            );
            Ok(())
        })
    }

    fn verify<'a>(
        &'a mut self,
        _node_id: &'a GraphNodeId,
        state: &'a mut Self::WorkingState,
        _effects: VerifyEffects<'a>,
    ) -> GraphFuture<'a, Result<VerificationOutcome, GraphProgramError>> {
        self.callbacks.fetch_add(1, Ordering::SeqCst);
        Box::pin(std::future::ready(Ok(
            if matches!(state.result, Some(ToolResult::Succeeded { .. })) {
                VerificationOutcome::Passed
            } else {
                VerificationOutcome::Failed
            },
        )))
    }

    fn decide<'a>(
        &'a mut self,
        _node_id: &'a GraphNodeId,
        _state: &'a mut Self::WorkingState,
        _context: DecisionContext<'a>,
    ) -> GraphFuture<'a, Result<agent_core::GraphBranchId, GraphProgramError>> {
        Box::pin(std::future::ready(Err(GraphProgramError::Failed)))
    }
}

impl RestartableGraphProgram for PocProgram {
    const RECOVERY_VERSION: u32 = 1;

    fn restore_working_state(
        &mut self,
        _state: &agent_harness::DurableRunState,
    ) -> Result<Self::WorkingState, GraphProgramError> {
        Ok(PocState::default())
    }
}

fn knowledge_request() -> KnowledgeRequest {
    KnowledgeRequest::new(
        KnowledgeQuery::new("ISO charging requirements").expect("query"),
        KnowledgeRoute::single(KnowledgeBackendId::StandardsMcp),
        RetrievalLimits::default(),
    )
}

fn evidence_set() -> EvidenceSet {
    let backend = KnowledgeBackendId::StandardsMcp;
    let evidence = Evidence::new(
        EvidenceId::new("standards:chunk-1").expect("evidence ID"),
        EvidenceSource::new(backend, "ISO15118-20").expect("source"),
        "bounded untrusted evidence".to_owned(),
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
            page_range: None,
            chunk_type: "text".to_owned(),
        },
    )
    .expect("evidence");
    EvidenceSet::from_backend(
        BackendEvidenceSet::new(backend, vec![evidence], None, false).expect("backend evidence"),
        RetrievalLimits::default(),
    )
    .expect("evidence set")
}

struct PendingKnowledge {
    calls: AtomicUsize,
}

impl PendingKnowledge {
    const fn new() -> Self {
        Self {
            calls: AtomicUsize::new(0),
        }
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

impl KnowledgePort for PendingKnowledge {
    fn retrieve<'a>(
        &'a self,
        _request: KnowledgeRequest,
    ) -> KnowledgeFuture<'a, Result<EvidenceSet, KnowledgeError>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Box::pin(pending())
    }
}

fn graph_budget(steps: u32) -> RunBudget {
    RunBudget::new(1, 1, 0, Duration::from_secs(30))
        .expect("base budget")
        .with_max_graph_steps(steps)
        .expect("graph budget")
}

#[tokio::test]
async fn poc_flow_uses_harness_governed_knowledge_model_and_action_effects() {
    let definition = linear_definition(false);
    let knowledge = Arc::new(CountingKnowledge::new(evidence_set()));
    let model = Arc::new(FakeModelPort::scripted(vec![Ok(ModelResponse::new(
        vec![ModelOutputPart::Text(
            r#"{"action":{"tool":"lookup","arguments":{}}}"#.to_owned(),
        )],
        None,
    ))]));
    let tool_definition = ToolDefinition::new(
        ToolName::new("lookup").expect("tool name"),
        "bounded read-only lookup",
        CapabilityKind::ReadOnly,
        ToolSchema::new(serde_json::json!({
            "type": "object",
            "additionalProperties": false
        }))
        .expect("schema"),
    )
    .expect("tool definition");
    let tool = Arc::new(FakeToolPort::succeeding(
        tool_definition,
        ToolOutput::new(serde_json::json!({"ok": true})),
    ));
    let mut registry = ToolRegistry::new();
    let tool_port: Arc<dyn ToolPort> = tool.clone();
    registry.register(tool_port).expect("register tool");
    let model_port: Arc<dyn ModelPort> = model.clone();
    let harness = ExecutionHarness::new(
        model_port,
        registry,
        Arc::new(M0ReadOnlyPolicy),
        Arc::new(InMemoryAuditSink::new()),
        HarnessConfig::new(AuditFailurePolicy::FailClosed, Duration::from_secs(1)).expect("config"),
    )
    .with_knowledge_port(knowledge.clone());
    let mut context = RunContext::new_graph(
        RunId::new(),
        SessionId::new(),
        graph_budget(5),
        1,
        definition.digest(),
    );
    let _cancellation = harness.start_run(&mut context).await.expect("start run");
    let callbacks = Arc::new(AtomicUsize::new(0));
    let mut program = PocProgram {
        callbacks: callbacks.clone(),
    };
    let mut state = PocState::default();

    let summary = crate::GraphEngine::new(&definition)
        .run(&harness, &mut context, &mut program, &mut state)
        .await
        .expect("graph run");

    assert_eq!(summary.terminal(), GraphTerminalOutcome::Complete);
    assert_eq!(summary.steps(), 5);
    assert_eq!(context.usage().iterations(), 0);
    assert_eq!(knowledge.calls(), 1);
    assert_eq!(model.invocation_count(), 1);
    assert_eq!(tool.invocation_count(), 1);
    assert_eq!(callbacks.load(Ordering::SeqCst), 4);
}

#[tokio::test]
async fn graph_action_preserves_m6_approval_and_containment_authority() {
    let definition = linear_definition(false);
    let knowledge = Arc::new(CountingKnowledge::new(evidence_set()));
    let model = Arc::new(FakeModelPort::scripted(vec![Ok(ModelResponse::new(
        vec![ModelOutputPart::Text(
            r#"{"action":{"tool":"workspace_write_file","arguments":{"relative_path":"report.txt","content":"bounded"}}}"#.to_owned(),
        )],
        None,
    ))]));
    let tool_definition = ToolDefinition::new(
        ToolName::new("workspace_write_file").expect("tool name"),
        "contained local workspace write",
        CapabilityKind::LocalWrite,
        ToolSchema::new(serde_json::json!({
            "type": "object",
            "properties": {
                "relative_path": {"type": "string"},
                "content": {"type": "string"}
            },
            "required": ["relative_path", "content"],
            "additionalProperties": false
        }))
        .expect("schema"),
    )
    .expect("tool definition");
    let contained = Arc::new(FakeContainedToolPort::succeeding(
        tool_definition,
        ToolOutput::new(serde_json::json!({"written": true})),
    ));
    let mut registry = ToolRegistry::new();
    let contained_port: Arc<dyn ContainedToolPort> = contained.clone();
    registry
        .register_contained(contained_port)
        .expect("register contained tool");
    let approval = Arc::new(ScriptedApprovalPort::approve_all());
    let audit = Arc::new(InMemoryAuditSink::new());
    let model_port: Arc<dyn ModelPort> = model.clone();
    let harness = ExecutionHarness::new(
        model_port,
        registry,
        Arc::new(M6ApprovalPolicy),
        audit.clone(),
        HarnessConfig::new(AuditFailurePolicy::FailClosed, Duration::from_secs(1)).expect("config"),
    )
    .with_approval_port(approval.clone())
    .with_knowledge_port(knowledge);
    let budget = graph_budget(5).with_max_approval_requests(1);
    let mut context = RunContext::new_graph(
        RunId::new(),
        SessionId::new(),
        budget,
        PocProgram::RECOVERY_VERSION,
        definition.digest(),
    );
    harness.start_run(&mut context).await.expect("start run");
    let mut program = PocProgram {
        callbacks: Arc::new(AtomicUsize::new(0)),
    };
    let summary = crate::GraphEngine::new(&definition)
        .run(
            &harness,
            &mut context,
            &mut program,
            &mut PocState::default(),
        )
        .await
        .expect("governed graph run");

    assert_eq!(summary.terminal(), GraphTerminalOutcome::Complete);
    assert_eq!(approval.invocation_count(), 1);
    assert_eq!(contained.preview_count(), 1);
    assert_eq!(contained.invocation_count(), 1);
    assert_eq!(context.usage().approval_requests(), 1);
    assert_eq!(context.usage().tool_calls(), 1);
    let events = audit.events();
    let tool_call_id = events.iter().find_map(|event| match event.kind() {
        AgentEventKind::ApprovalRequested { tool_call_id, .. } => Some(*tool_call_id),
        _ => None,
    });
    assert!(tool_call_id.is_some());
    assert!(events.iter().any(|event| matches!(
        event.kind(),
        AgentEventKind::ToolInvocationStarted {
            tool_call_id: current,
            capability: CapabilityKind::LocalWrite,
            ..
        } if Some(*current) == tool_call_id
    )));
    assert!(events.iter().any(|event| matches!(
        event.kind(),
        AgentEventKind::ToolInvocationCompleted { tool_call_id: current }
            if Some(*current) == tool_call_id
    )));
}

#[tokio::test]
async fn sqlite_replay_is_inert_and_resume_retrieves_fresh_without_duplicate_effects() {
    let definition = linear_definition(false);
    let directory = tempfile::tempdir().expect("temporary directory");
    let store = Arc::new(
        agent_persistence_sqlite::SqliteRunPersistence::open(directory.path().join("runs.sqlite3"))
            .await
            .expect("store"),
    );
    let pending_knowledge = Arc::new(PendingKnowledge::new());
    let first_harness = effect_free_harness()
        .with_knowledge_port(pending_knowledge.clone())
        .with_persistence_port(store.clone());
    let run_id = RunId::new();
    let session_id = SessionId::new();
    let mut context = RunContext::new_graph(
        run_id,
        session_id,
        RunBudget::new(1, 1, 0, Duration::from_secs(30))
            .expect("budget")
            .with_max_graph_steps(6)
            .expect("steps"),
        PocProgram::RECOVERY_VERSION,
        definition.digest(),
    );
    first_harness.start_run(&mut context).await.expect("start");
    let mut first_program = PocProgram {
        callbacks: Arc::new(AtomicUsize::new(0)),
    };
    let mut first_state = PocState::default();
    let interrupted = tokio::time::timeout(
        Duration::from_millis(500),
        crate::GraphEngine::new(&definition).run(
            &first_harness,
            &mut context,
            &mut first_program,
            &mut first_state,
        ),
    )
    .await;
    assert!(interrupted.is_err());
    assert_eq!(pending_knowledge.calls(), 1);

    let knowledge = Arc::new(CountingKnowledge::new(evidence_set()));
    let model = Arc::new(FakeModelPort::scripted(vec![Ok(ModelResponse::new(
        vec![ModelOutputPart::Text(
            r#"{"action":{"tool":"lookup","arguments":{}}}"#.to_owned(),
        )],
        None,
    ))]));
    let tool_definition = ToolDefinition::new(
        ToolName::new("lookup").expect("tool name"),
        "bounded read-only lookup",
        CapabilityKind::ReadOnly,
        ToolSchema::new(serde_json::json!({
            "type": "object",
            "additionalProperties": false
        }))
        .expect("schema"),
    )
    .expect("tool definition");
    let tool = Arc::new(FakeToolPort::succeeding(
        tool_definition,
        ToolOutput::new(serde_json::json!({"ok": true})),
    ));
    let mut registry = ToolRegistry::new();
    let tool_port: Arc<dyn ToolPort> = tool.clone();
    registry.register(tool_port).expect("register tool");
    let model_port: Arc<dyn ModelPort> = model.clone();
    let second_harness = ExecutionHarness::new(
        model_port,
        registry,
        Arc::new(M0ReadOnlyPolicy),
        Arc::new(InMemoryAuditSink::new()),
        HarnessConfig::new(AuditFailurePolicy::FailClosed, Duration::from_secs(1)).expect("config"),
    )
    .with_knowledge_port(knowledge.clone())
    .with_persistence_port(store);

    let disposition = second_harness
        .recover_run(RunKey::new(run_id, session_id))
        .await
        .expect("recover");
    assert_eq!(knowledge.calls(), 0);
    assert_eq!(model.invocation_count(), 0);
    assert_eq!(tool.invocation_count(), 0);
    let RecoveryDisposition::Resumable(recovered) = disposition else {
        panic!("interrupted Retrieve must be resumable");
    };
    let mut resumed_program = PocProgram {
        callbacks: Arc::new(AtomicUsize::new(0)),
    };
    let (summary, _) = crate::GraphEngine::new(&definition)
        .resume(&second_harness, *recovered, &mut resumed_program)
        .await
        .expect("fresh resume");

    assert_eq!(summary.terminal(), GraphTerminalOutcome::Complete);
    assert_eq!(summary.steps(), 6);
    assert_eq!(knowledge.calls(), 1);
    assert_eq!(model.invocation_count(), 1);
    assert_eq!(tool.invocation_count(), 1);
}

#[tokio::test]
async fn resume_rejects_program_version_mismatch_before_callbacks() {
    let definition = linear_definition(false);
    let directory = tempfile::tempdir().expect("temporary directory");
    let store = Arc::new(
        agent_persistence_sqlite::SqliteRunPersistence::open(directory.path().join("runs.sqlite3"))
            .await
            .expect("store"),
    );
    let knowledge = Arc::new(PendingKnowledge::new());
    let harness = effect_free_harness()
        .with_knowledge_port(knowledge.clone())
        .with_persistence_port(store);
    let run_id = RunId::new();
    let session_id = SessionId::new();
    let mut context = RunContext::new_graph(
        run_id,
        session_id,
        RunBudget::new(1, 1, 0, Duration::from_secs(30))
            .expect("budget")
            .with_max_graph_steps(2)
            .expect("steps"),
        PocProgram::RECOVERY_VERSION + 1,
        definition.digest(),
    );
    harness.start_run(&mut context).await.expect("start");
    let callbacks = Arc::new(AtomicUsize::new(0));
    let mut first_program = PocProgram {
        callbacks: callbacks.clone(),
    };
    let mut first_state = PocState::default();
    assert!(
        tokio::time::timeout(
            Duration::from_millis(500),
            crate::GraphEngine::new(&definition).run(
                &harness,
                &mut context,
                &mut first_program,
                &mut first_state,
            )
        )
        .await
        .is_err()
    );

    let disposition = harness
        .recover_run(RunKey::new(run_id, session_id))
        .await
        .expect("recover");
    let RecoveryDisposition::Resumable(recovered) = disposition else {
        panic!("interrupted Retrieve must be resumable");
    };
    let resumed_callbacks = Arc::new(AtomicUsize::new(0));
    let mut resumed_program = PocProgram {
        callbacks: resumed_callbacks.clone(),
    };
    let result = crate::GraphEngine::new(&definition)
        .resume(&harness, *recovered, &mut resumed_program)
        .await;

    assert!(matches!(result, Err(crate::GraphError::RecoveryMismatch)));
    assert_eq!(knowledge.calls(), 1);
    assert_eq!(callbacks.load(Ordering::SeqCst), 1);
    assert_eq!(resumed_callbacks.load(Ordering::SeqCst), 0);
}

struct FailingProgram {
    calls: Arc<AtomicUsize>,
}

impl GraphProgram for FailingProgram {
    type WorkingState = ();

    fn retrieve<'a>(
        &'a mut self,
        _node_id: &'a GraphNodeId,
        _state: &'a mut Self::WorkingState,
        _effects: RetrieveEffects<'a>,
    ) -> GraphFuture<'a, Result<(), GraphProgramError>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Box::pin(std::future::ready(Err(GraphProgramError::Failed)))
    }

    fn model<'a>(
        &'a mut self,
        _node_id: &'a GraphNodeId,
        _state: &'a mut Self::WorkingState,
        _effects: ModelEffects<'a>,
    ) -> GraphFuture<'a, Result<(), GraphProgramError>> {
        Box::pin(std::future::ready(Err(GraphProgramError::Failed)))
    }

    fn action<'a>(
        &'a mut self,
        _node_id: &'a GraphNodeId,
        _state: &'a mut Self::WorkingState,
        _effects: ActionEffects<'a>,
    ) -> GraphFuture<'a, Result<(), GraphProgramError>> {
        Box::pin(std::future::ready(Err(GraphProgramError::Failed)))
    }

    fn verify<'a>(
        &'a mut self,
        _node_id: &'a GraphNodeId,
        _state: &'a mut Self::WorkingState,
        _effects: VerifyEffects<'a>,
    ) -> GraphFuture<'a, Result<VerificationOutcome, GraphProgramError>> {
        Box::pin(std::future::ready(Err(GraphProgramError::Failed)))
    }

    fn decide<'a>(
        &'a mut self,
        _node_id: &'a GraphNodeId,
        _state: &'a mut Self::WorkingState,
        _context: DecisionContext<'a>,
    ) -> GraphFuture<'a, Result<agent_core::GraphBranchId, GraphProgramError>> {
        Box::pin(std::future::ready(Err(GraphProgramError::Failed)))
    }
}

fn effect_free_harness() -> ExecutionHarness {
    ExecutionHarness::new(
        Arc::new(FakeModelPort::scripted(Vec::new())),
        ToolRegistry::new(),
        Arc::new(M0ReadOnlyPolicy),
        Arc::new(InMemoryAuditSink::new()),
        HarnessConfig::new(AuditFailurePolicy::FailClosed, Duration::from_secs(1)).expect("config"),
    )
}

#[tokio::test]
async fn callback_failure_terminalizes_once_without_retry() {
    let definition = GraphDefinition::new(
        id("retrieve"),
        vec![
            node("retrieve", NodeKind::Retrieve),
            node("complete", NodeKind::Complete),
        ],
        vec![edge("retrieve", GraphTransitionKey::Succeeded, "complete")],
    )
    .expect("definition");
    let harness = effect_free_harness();
    let mut context = RunContext::new_graph(
        RunId::new(),
        SessionId::new(),
        RunBudget::new(0, 0, 0, Duration::from_secs(30))
            .expect("budget")
            .with_max_graph_steps(2)
            .expect("steps"),
        1,
        definition.digest(),
    );
    harness.start_run(&mut context).await.expect("start");
    let calls = Arc::new(AtomicUsize::new(0));
    let mut program = FailingProgram {
        calls: calls.clone(),
    };

    let result = crate::GraphEngine::new(&definition)
        .run(&harness, &mut context, &mut program, &mut ())
        .await;

    assert!(matches!(result, Err(crate::GraphError::Program(_))));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert!(matches!(
        context.status(),
        agent_core::RunStatus::Finished(_)
    ));
}

struct RejectGraphStepPersistence;

impl RunPersistencePort for RejectGraphStepPersistence {
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
        let result = if matches!(
            transition.event().kind(),
            AgentEventKind::Graph {
                event: agent_core::GraphProgressEvent::GraphNodeEntered { .. }
            }
        ) {
            Err(PersistencePortError::Unavailable)
        } else {
            Ok(())
        };
        Box::pin(std::future::ready(result))
    }

    fn load_run<'a>(
        &'a self,
        _key: RunKey,
    ) -> PersistenceFuture<'a, Result<LoadedRun, PersistencePortError>> {
        Box::pin(std::future::ready(Err(PersistencePortError::Unavailable)))
    }
}

#[tokio::test]
async fn graph_step_persistence_failure_invokes_no_callback() {
    let definition = GraphDefinition::new(
        id("retrieve"),
        vec![
            node("retrieve", NodeKind::Retrieve),
            node("complete", NodeKind::Complete),
        ],
        vec![edge("retrieve", GraphTransitionKey::Succeeded, "complete")],
    )
    .expect("definition");
    let harness = effect_free_harness().with_persistence_port(Arc::new(RejectGraphStepPersistence));
    let mut context = RunContext::new_graph(
        RunId::new(),
        SessionId::new(),
        RunBudget::new(0, 0, 0, Duration::from_secs(30))
            .expect("budget")
            .with_max_graph_steps(2)
            .expect("steps"),
        1,
        definition.digest(),
    );
    harness.start_run(&mut context).await.expect("start");
    let calls = Arc::new(AtomicUsize::new(0));
    let mut program = FailingProgram {
        calls: calls.clone(),
    };

    let result = crate::GraphEngine::new(&definition)
        .run(&harness, &mut context, &mut program, &mut ())
        .await;

    assert!(matches!(result, Err(crate::GraphError::Harness(_))));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

struct BudgetProgram {
    retrieve_calls: Arc<AtomicUsize>,
    model_calls: Arc<AtomicUsize>,
}

impl GraphProgram for BudgetProgram {
    type WorkingState = ();

    fn retrieve<'a>(
        &'a mut self,
        _node_id: &'a GraphNodeId,
        _state: &'a mut Self::WorkingState,
        _effects: RetrieveEffects<'a>,
    ) -> GraphFuture<'a, Result<(), GraphProgramError>> {
        self.retrieve_calls.fetch_add(1, Ordering::SeqCst);
        Box::pin(std::future::ready(Ok(())))
    }

    fn model<'a>(
        &'a mut self,
        _node_id: &'a GraphNodeId,
        _state: &'a mut Self::WorkingState,
        _effects: ModelEffects<'a>,
    ) -> GraphFuture<'a, Result<(), GraphProgramError>> {
        self.model_calls.fetch_add(1, Ordering::SeqCst);
        Box::pin(std::future::ready(Ok(())))
    }

    fn action<'a>(
        &'a mut self,
        _node_id: &'a GraphNodeId,
        _state: &'a mut Self::WorkingState,
        _effects: ActionEffects<'a>,
    ) -> GraphFuture<'a, Result<(), GraphProgramError>> {
        Box::pin(std::future::ready(Err(GraphProgramError::Failed)))
    }

    fn verify<'a>(
        &'a mut self,
        _node_id: &'a GraphNodeId,
        _state: &'a mut Self::WorkingState,
        _effects: VerifyEffects<'a>,
    ) -> GraphFuture<'a, Result<VerificationOutcome, GraphProgramError>> {
        Box::pin(std::future::ready(Err(GraphProgramError::Failed)))
    }

    fn decide<'a>(
        &'a mut self,
        _node_id: &'a GraphNodeId,
        _state: &'a mut Self::WorkingState,
        _context: DecisionContext<'a>,
    ) -> GraphFuture<'a, Result<agent_core::GraphBranchId, GraphProgramError>> {
        Box::pin(std::future::ready(Err(GraphProgramError::Failed)))
    }
}

#[tokio::test]
async fn graph_step_exhaustion_happens_before_the_next_callback() {
    let definition = GraphDefinition::new(
        id("retrieve"),
        vec![
            node("retrieve", NodeKind::Retrieve),
            node("model", NodeKind::Model),
            node("complete", NodeKind::Complete),
        ],
        vec![
            edge("retrieve", GraphTransitionKey::Succeeded, "model"),
            edge("model", GraphTransitionKey::Succeeded, "complete"),
        ],
    )
    .expect("definition");
    let harness = effect_free_harness();
    let mut context = RunContext::new_graph(
        RunId::new(),
        SessionId::new(),
        RunBudget::new(0, 0, 0, Duration::from_secs(30))
            .expect("budget")
            .with_max_graph_steps(1)
            .expect("steps"),
        1,
        definition.digest(),
    );
    harness.start_run(&mut context).await.expect("start");
    let retrieve_calls = Arc::new(AtomicUsize::new(0));
    let model_calls = Arc::new(AtomicUsize::new(0));
    let mut program = BudgetProgram {
        retrieve_calls: retrieve_calls.clone(),
        model_calls: model_calls.clone(),
    };

    let result = crate::GraphEngine::new(&definition)
        .run(&harness, &mut context, &mut program, &mut ())
        .await;

    assert!(matches!(result, Err(crate::GraphError::Harness(_))));
    assert_eq!(retrieve_calls.load(Ordering::SeqCst), 1);
    assert_eq!(model_calls.load(Ordering::SeqCst), 0);
    assert_eq!(context.usage().graph_steps(), 1);
    assert!(matches!(
        context.status(),
        RunStatus::Finished(RunOutcome::BudgetExceeded {
            dimension: BudgetDimension::GraphSteps
        })
    ));
}

#[test]
fn graph_crate_does_not_depend_on_direct_authority_or_provider_crates() {
    let production = [
        include_str!("definition.rs"),
        include_str!("effects.rs"),
        include_str!("engine.rs"),
        include_str!("error.rs"),
        include_str!("program.rs"),
    ]
    .join("\n");
    for forbidden in [
        "ModelPort",
        "KnowledgePort",
        "ToolPort",
        "ApprovalPort",
        "ContainedToolPort",
        "ToolRegistry",
        "CapabilityPolicy",
    ] {
        assert!(!production.contains(forbidden), "found {forbidden}");
    }
    let manifest = include_str!("../Cargo.toml");
    for forbidden in ["agent-loop", "agent-provider-rig", "rmcp", "reqwest"] {
        assert!(!manifest.contains(forbidden), "found {forbidden}");
    }
}
