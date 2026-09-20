use crate::{
    ActionEffects, DecisionContext, Edge, GraphBranchId, GraphDefinition, GraphDefinitionError,
    GraphFuture, GraphNodeId, GraphProgram, GraphProgramError, GraphRecoveryMode,
    GraphTerminalOutcome, GraphTransitionKey, MAX_GRAPH_NODES, ModelEffects, NodeDefinition,
    NodeKind, RestartableGraphProgram, RetrieveEffects, VerificationOutcome, VerifyEffects,
};
use agent_action_seal_local::LocalActionSealer;
use agent_core::{
    ActionDigest, ActionProposalId, AgentEventKind, ApprovalRequestId, BudgetDimension,
    CapabilityKind, DurableApprovalOutcome, KnowledgeBackendId, ModelMessage, ModelOutputPart,
    ModelRequest, ModelResponse, ModelRole, RunBudget, RunId, RunOutcome, RunStatus, SessionId,
    ToolCall, ToolCallId, ToolDefinition, ToolName, ToolOutput, ToolResult, ToolSchema,
    WorkspaceBindingId,
};
use agent_harness::{
    ActionSealBinding, ActionSealError, ActionSealPort, AppendTransition, ApprovalPreview,
    AuditFailurePolicy, CompletedKnowledgeRetrieval, CompletedModelInvocation, ContainedInvocation,
    ContainedToolPort, ContainmentPortError, DurableApprovalDecisionCommand, DurableApprovalWait,
    DurableCheckpoint, ExecutionHarness, HarnessConfig, LoadedRun, LocalWriteActionCapsuleV1,
    M0ReadOnlyPolicy, M6ApprovalPolicy, ModelPort, PersistenceFuture, PersistencePortError,
    RecoveryDisposition, RunContext, RunKey, RunPersistencePort, RunReadPort, RunRecord,
    SealedLocalWriteAction, ToolPort, ToolRegistry,
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

struct CountingSeal {
    inner: LocalActionSealer,
    seals: AtomicUsize,
    opens: AtomicUsize,
}

impl CountingSeal {
    fn new() -> Self {
        Self {
            inner: LocalActionSealer::new("m10-key", [9; 32]).expect("sealer"),
            seals: AtomicUsize::new(0),
            opens: AtomicUsize::new(0),
        }
    }

    fn opens(&self) -> usize {
        self.opens.load(Ordering::SeqCst)
    }
}

impl ActionSealPort for CountingSeal {
    fn seal_local_write(
        &self,
        binding: &ActionSealBinding,
        action: &LocalWriteActionCapsuleV1,
    ) -> Result<SealedLocalWriteAction, ActionSealError> {
        self.seals.fetch_add(1, Ordering::SeqCst);
        self.inner.seal_local_write(binding, action)
    }

    fn open_local_write(
        &self,
        binding: &ActionSealBinding,
        sealed: &SealedLocalWriteAction,
    ) -> Result<LocalWriteActionCapsuleV1, ActionSealError> {
        self.opens.fetch_add(1, Ordering::SeqCst);
        self.inner.open_local_write(binding, sealed)
    }
}

struct DurablePreviewTool {
    inner: FakeContainedToolPort,
}

impl DurablePreviewTool {
    fn new(definition: ToolDefinition) -> Self {
        Self {
            inner: FakeContainedToolPort::succeeding(
                definition,
                ToolOutput::new(serde_json::json!({"written": true})),
            ),
        }
    }

    fn invocation_count(&self) -> usize {
        self.inner.invocation_count()
    }
}

impl ContainedToolPort for DurablePreviewTool {
    fn definition(&self) -> &ToolDefinition {
        self.inner.definition()
    }

    fn approval_preview(
        &self,
        input: &agent_core::ToolInput,
    ) -> Result<ApprovalPreview, ContainmentPortError> {
        let object = input
            .as_value()
            .as_object()
            .ok_or(ContainmentPortError::PreviewRejected)?;
        let path = object
            .get("relative_path")
            .and_then(serde_json::Value::as_str)
            .ok_or(ContainmentPortError::PreviewRejected)?;
        let content = object
            .get("content")
            .and_then(serde_json::Value::as_str)
            .ok_or(ContainmentPortError::PreviewRejected)?;
        ApprovalPreview::new(
            format!("workspace_write_file ({} bytes)", content.len()),
            path,
        )
        .map_err(ContainmentPortError::from)
    }

    fn start_contained(
        &self,
        call: ToolCall,
    ) -> Result<Box<dyn ContainedInvocation>, ContainmentPortError> {
        self.inner.start_contained(call)
    }
}

#[derive(Default)]
struct DurableState {
    model: Option<CompletedModelInvocation>,
}

struct DurableProgram {
    action_callbacks: Arc<AtomicUsize>,
}

impl GraphProgram for DurableProgram {
    type WorkingState = DurableState;

    fn retrieve<'a>(
        &'a mut self,
        _node_id: &'a GraphNodeId,
        _state: &'a mut Self::WorkingState,
        _effects: RetrieveEffects<'a>,
    ) -> GraphFuture<'a, Result<(), GraphProgramError>> {
        Box::pin(std::future::ready(Err(GraphProgramError::Failed)))
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
                        "prepare local write",
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
        Box::pin(std::future::ready(Err(GraphProgramError::Failed)))
    }

    fn durable_local_write_action<'a>(
        &'a mut self,
        _node_id: &'a GraphNodeId,
        state: &'a mut Self::WorkingState,
        mut effects: ActionEffects<'a>,
    ) -> GraphFuture<'a, Result<DurableApprovalWait, GraphProgramError>> {
        self.action_callbacks.fetch_add(1, Ordering::SeqCst);
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
        Box::pin(std::future::ready(Ok(VerificationOutcome::Passed)))
    }

    fn decide<'a>(
        &'a mut self,
        _node_id: &'a GraphNodeId,
        _state: &'a mut Self::WorkingState,
        _context: DecisionContext<'a>,
    ) -> GraphFuture<'a, Result<GraphBranchId, GraphProgramError>> {
        Box::pin(std::future::ready(Err(GraphProgramError::Failed)))
    }
}

impl RestartableGraphProgram for DurableProgram {
    const RECOVERY_VERSION: u32 = 10;

    fn restore_working_state(
        &mut self,
        _state: &agent_harness::DurableRunState,
    ) -> Result<Self::WorkingState, GraphProgramError> {
        Ok(DurableState::default())
    }
}

fn durable_definition() -> GraphDefinition {
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
            edge("model", GraphTransitionKey::Succeeded, "action"),
            edge("action", GraphTransitionKey::Succeeded, "verify"),
            edge("action", GraphTransitionKey::ApprovalDenied, "denied"),
            edge("verify", GraphTransitionKey::VerificationPassed, "complete"),
            edge("verify", GraphTransitionKey::VerificationFailed, "denied"),
        ],
    )
    .expect("durable graph")
}

fn durable_tool_definition() -> ToolDefinition {
    ToolDefinition::new(
        ToolName::new("workspace_write_file").expect("tool name"),
        "write one bounded UTF-8 file beneath the configured workspace",
        CapabilityKind::LocalWrite,
        ToolSchema::new(serde_json::json!({
            "type": "object",
            "properties": {
                "relative_path": {"type": "string", "maxLength": 240},
                "content": {"type": "string", "maxLength": 4096}
            },
            "required": ["relative_path", "content"],
            "additionalProperties": false
        }))
        .expect("schema"),
    )
    .expect("definition")
}

fn durable_harness(
    model: Arc<FakeModelPort>,
    tool: Arc<DurablePreviewTool>,
    store: Arc<agent_persistence_sqlite::SqliteRunPersistence>,
    seal: Arc<dyn ActionSealPort>,
    workspace: WorkspaceBindingId,
    audit: Arc<InMemoryAuditSink>,
) -> ExecutionHarness {
    let mut registry = ToolRegistry::new();
    let contained: Arc<dyn ContainedToolPort> = tool;
    registry.register_contained(contained).expect("register");
    let model: Arc<dyn ModelPort> = model;
    ExecutionHarness::new(
        model,
        registry,
        Arc::new(M6ApprovalPolicy),
        audit,
        HarnessConfig::new(AuditFailurePolicy::FailClosed, Duration::from_secs(1)).expect("config"),
    )
    .with_persistence_port(store)
    .with_durable_local_write_approval(seal, workspace)
}

#[tokio::test]
async fn durable_local_write_waits_restarts_previews_and_resumes_once() {
    let definition = durable_definition();
    let directory = tempfile::tempdir().expect("directory");
    let store = Arc::new(
        agent_persistence_sqlite::SqliteRunPersistence::open(directory.path().join("m10.sqlite3"))
            .await
            .expect("store"),
    );
    let counting_seal = Arc::new(CountingSeal::new());
    let sealer: Arc<dyn ActionSealPort> = counting_seal.clone();
    let workspace = WorkspaceBindingId::new();
    let tool = Arc::new(DurablePreviewTool::new(durable_tool_definition()));
    let first_model = Arc::new(FakeModelPort::scripted(vec![Ok(ModelResponse::new(
        vec![ModelOutputPart::Text(
            r#"{"action":{"tool":"workspace_write_file","arguments":{"relative_path":"report.txt","content":"private-content"}}}"#.to_owned(),
        )],
        None,
    ))]));
    let audit = Arc::new(InMemoryAuditSink::new());
    let first = durable_harness(
        first_model.clone(),
        tool.clone(),
        store.clone(),
        sealer.clone(),
        workspace,
        audit.clone(),
    );
    let run_id = RunId::new();
    let session_id = SessionId::new();
    let budget = RunBudget::new(1, 1, 0, Duration::from_secs(30))
        .expect("budget")
        .with_max_approval_requests(1)
        .with_max_graph_steps(4)
        .expect("steps");
    let mut context = RunContext::new_graph(
        run_id,
        session_id,
        budget,
        DurableProgram::RECOVERY_VERSION,
        definition.digest(),
    );
    first.start_run(&mut context).await.expect("start");
    let action_callbacks = Arc::new(AtomicUsize::new(0));
    let mut program = DurableProgram {
        action_callbacks: action_callbacks.clone(),
    };
    let waiting = crate::GraphEngine::new(&definition)
        .run(
            &first,
            &mut context,
            &mut program,
            &mut DurableState::default(),
        )
        .await
        .expect("suspend");
    assert_eq!(waiting.terminal(), GraphTerminalOutcome::Waiting);
    let wait = waiting.waiting().expect("wait handle");
    assert_eq!(context.usage().approval_requests(), 1);
    assert_eq!(context.usage().tool_calls(), 0);
    assert_eq!(tool.invocation_count(), 0);
    assert_eq!(action_callbacks.load(Ordering::SeqCst), 1);
    assert_eq!(
        store
            .load_pending_approval_requests(RunKey::new(run_id, session_id))
            .await
            .expect("pending waits")
            .len(),
        1
    );
    let database_bytes = std::fs::read(store.path()).expect("read SQLite database");
    assert!(
        !database_bytes
            .windows(b"private-content".len())
            .any(|window| window == b"private-content")
    );
    assert!(
        !database_bytes
            .windows(b"report.txt".len())
            .any(|window| window == b"report.txt")
    );

    let wrong_workspace = durable_harness(
        Arc::new(FakeModelPort::scripted(Vec::new())),
        tool.clone(),
        store.clone(),
        sealer.clone(),
        WorkspaceBindingId::new(),
        audit.clone(),
    );
    assert!(
        wrong_workspace
            .durable_approval_view(RunKey::new(run_id, session_id), wait.wait_id())
            .await
            .is_err()
    );
    assert_eq!(counting_seal.opens(), 0);

    let mismatched_definition = ToolDefinition::new(
        ToolName::new("workspace_write_file").expect("tool name"),
        "changed trusted contract",
        CapabilityKind::LocalWrite,
        durable_tool_definition().input_schema().clone(),
    )
    .expect("mismatched definition");
    let wrong_contract = durable_harness(
        Arc::new(FakeModelPort::scripted(Vec::new())),
        Arc::new(DurablePreviewTool::new(mismatched_definition)),
        store.clone(),
        sealer.clone(),
        workspace,
        audit.clone(),
    );
    assert!(
        wrong_contract
            .durable_approval_view(RunKey::new(run_id, session_id), wait.wait_id())
            .await
            .is_err()
    );
    assert_eq!(counting_seal.opens(), 0);

    let second_model = Arc::new(FakeModelPort::scripted(Vec::new()));
    let second = durable_harness(
        second_model.clone(),
        tool.clone(),
        store.clone(),
        sealer,
        workspace,
        audit.clone(),
    );
    let key = RunKey::new(run_id, session_id);
    let disposition = second.recover_run(key).await.expect("recover waiting");
    assert!(matches!(disposition, RecoveryDisposition::Waiting(_)));
    assert_eq!(counting_seal.opens(), 0);
    assert_eq!(second_model.invocation_count(), 0);
    assert_eq!(tool.invocation_count(), 0);

    let view = second
        .durable_approval_view(key, wait.wait_id())
        .await
        .expect("preview");
    assert_eq!(view.preview().target_label(), "report.txt");
    assert_eq!(view.preview().summary(), "workspace_write_file (15 bytes)");
    assert!(!view.preview().summary().contains("private-content"));
    assert_eq!(counting_seal.opens(), 1);
    let wrong = second
        .record_durable_approval_decision(DurableApprovalDecisionCommand {
            key,
            wait_id: wait.wait_id(),
            approval_request_id: wait.approval_request_id(),
            action_proposal_id: wait.action_proposal_id(),
            tool_call_id: wait.tool_call_id(),
            action_digest: ActionDigest::from_bytes([0; 32]),
            expected_row_version: view.row_version(),
            outcome: DurableApprovalOutcome::Approve,
            actor: None,
            decided_at_unix_millis: None,
        })
        .await;
    assert!(wrong.is_err());
    let command = DurableApprovalDecisionCommand {
        key,
        wait_id: wait.wait_id(),
        approval_request_id: wait.approval_request_id(),
        action_proposal_id: wait.action_proposal_id(),
        tool_call_id: wait.tool_call_id(),
        action_digest: wait.action_digest(),
        expected_row_version: view.row_version(),
        outcome: DurableApprovalOutcome::Approve,
        actor: None,
        decided_at_unix_millis: None,
    };
    for wrong in [
        DurableApprovalDecisionCommand {
            approval_request_id: ApprovalRequestId::new(),
            ..command.clone()
        },
        DurableApprovalDecisionCommand {
            action_proposal_id: ActionProposalId::new(),
            ..command.clone()
        },
        DurableApprovalDecisionCommand {
            tool_call_id: ToolCallId::new(),
            ..command.clone()
        },
        DurableApprovalDecisionCommand {
            key: RunKey::new(RunId::new(), session_id),
            ..command.clone()
        },
    ] {
        assert!(
            second
                .record_durable_approval_decision(wrong)
                .await
                .is_err()
        );
    }
    let (left, right) = tokio::join!(
        second.record_durable_approval_outcome(
            key,
            wait.wait_id(),
            view.row_version(),
            DurableApprovalOutcome::Approve,
        ),
        second.record_durable_approval_outcome(
            key,
            wait.wait_id(),
            view.row_version(),
            DurableApprovalOutcome::Approve,
        ),
    );
    assert_ne!(left.is_ok(), right.is_ok());
    let duplicate = second.record_durable_approval_decision(command).await;
    assert!(duplicate.is_err());

    let RecoveryDisposition::Waiting(wrong_recovered) =
        second.recover_run(key).await.expect("recover decided wait")
    else {
        panic!("decision must remain intentional waiting until explicit resume");
    };
    let wrong_definition = linear_definition(false);
    let mismatch = crate::GraphEngine::new(&wrong_definition)
        .resume_waiting(
            &second,
            *wrong_recovered,
            wait.wait_id(),
            &mut DurableProgram {
                action_callbacks: action_callbacks.clone(),
            },
        )
        .await;
    assert!(matches!(mismatch, Err(crate::GraphError::RecoveryMismatch)));
    let RecoveryDisposition::Waiting(recovered) = second
        .recover_run(key)
        .await
        .expect("recover after mismatch")
    else {
        panic!("mismatch must not consume the wait");
    };
    let mut resumed_program = DurableProgram {
        action_callbacks: action_callbacks.clone(),
    };
    let (summary, _) = crate::GraphEngine::new(&definition)
        .resume_waiting(&second, *recovered, wait.wait_id(), &mut resumed_program)
        .await
        .expect("resume exact action");
    assert_eq!(summary.terminal(), GraphTerminalOutcome::Complete);
    assert_eq!(summary.steps(), 4);
    assert_eq!(tool.invocation_count(), 1);
    assert_eq!(second_model.invocation_count(), 0);
    assert_eq!(action_callbacks.load(Ordering::SeqCst), 1);
    assert_eq!(counting_seal.opens(), 2);
    assert!(
        store
            .load_pending_approval_requests(key)
            .await
            .expect("consumed waits")
            .is_empty()
    );
    assert!(audit.events().iter().any(|event| matches!(
        event.kind(),
        AgentEventKind::ToolInvocationStarted { tool_call_id, .. }
            if *tool_call_id == wait.tool_call_id()
    )));
}

#[tokio::test]
async fn durable_local_write_denial_after_restart_executes_zero_tools() {
    let definition = durable_definition();
    let directory = tempfile::tempdir().expect("directory");
    let store = Arc::new(
        agent_persistence_sqlite::SqliteRunPersistence::open(directory.path().join("deny.sqlite3"))
            .await
            .expect("store"),
    );
    let seal: Arc<dyn ActionSealPort> = Arc::new(CountingSeal::new());
    let workspace = WorkspaceBindingId::new();
    let tool = Arc::new(DurablePreviewTool::new(durable_tool_definition()));
    let model = Arc::new(FakeModelPort::scripted(vec![Ok(ModelResponse::new(
        vec![ModelOutputPart::Text(
            r#"{"action":{"tool":"workspace_write_file","arguments":{"relative_path":"denied.txt","content":"no-write"}}}"#.to_owned(),
        )],
        None,
    ))]));
    let audit = Arc::new(InMemoryAuditSink::new());
    let first = durable_harness(
        model,
        tool.clone(),
        store.clone(),
        seal.clone(),
        workspace,
        audit.clone(),
    );
    let run_id = RunId::new();
    let session_id = SessionId::new();
    let mut context = RunContext::new_graph(
        run_id,
        session_id,
        RunBudget::new(1, 1, 0, Duration::from_secs(30))
            .expect("budget")
            .with_max_approval_requests(1)
            .with_max_graph_steps(3)
            .expect("steps"),
        DurableProgram::RECOVERY_VERSION,
        definition.digest(),
    );
    first.start_run(&mut context).await.expect("start");
    let callbacks = Arc::new(AtomicUsize::new(0));
    let wait = crate::GraphEngine::new(&definition)
        .run(
            &first,
            &mut context,
            &mut DurableProgram {
                action_callbacks: callbacks.clone(),
            },
            &mut DurableState::default(),
        )
        .await
        .expect("wait")
        .waiting()
        .expect("wait handle");
    let second = durable_harness(
        Arc::new(FakeModelPort::scripted(Vec::new())),
        tool.clone(),
        store,
        seal,
        workspace,
        audit,
    );
    let key = RunKey::new(run_id, session_id);
    let view = second
        .durable_approval_view(key, wait.wait_id())
        .await
        .expect("view");
    second
        .record_durable_approval_decision(DurableApprovalDecisionCommand {
            key,
            wait_id: wait.wait_id(),
            approval_request_id: wait.approval_request_id(),
            action_proposal_id: wait.action_proposal_id(),
            tool_call_id: wait.tool_call_id(),
            action_digest: wait.action_digest(),
            expected_row_version: view.row_version(),
            outcome: DurableApprovalOutcome::Deny,
            actor: None,
            decided_at_unix_millis: None,
        })
        .await
        .expect("deny");
    let RecoveryDisposition::Waiting(recovered) = second.recover_run(key).await.expect("recover")
    else {
        panic!("denied decision remains waiting for explicit resume");
    };
    let (summary, _) = crate::GraphEngine::new(&definition)
        .resume_waiting(
            &second,
            *recovered,
            wait.wait_id(),
            &mut DurableProgram {
                action_callbacks: callbacks.clone(),
            },
        )
        .await
        .expect("resume denial");
    assert_eq!(summary.terminal(), GraphTerminalOutcome::Fail);
    assert_eq!(tool.invocation_count(), 0);
    assert_eq!(callbacks.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn durable_wait_transaction_failure_publishes_neither_wait_nor_suspension() {
    let definition = durable_definition();
    let directory = tempfile::tempdir().expect("directory");
    let store = Arc::new(
        agent_persistence_sqlite::SqliteRunPersistence::open(
            directory.path().join("wait-crash.sqlite3"),
        )
        .await
        .expect("store"),
    );
    let connection = rusqlite::Connection::open(store.path()).expect("database");
    connection
        .execute_batch(
            "CREATE TRIGGER fail_wait_insert BEFORE INSERT ON durable_approvals
             BEGIN SELECT RAISE(ABORT, 'injected crash'); END;",
        )
        .expect("failure trigger");
    drop(connection);

    let tool = Arc::new(DurablePreviewTool::new(durable_tool_definition()));
    let harness = durable_harness(
        Arc::new(FakeModelPort::scripted(vec![Ok(ModelResponse::new(
            vec![ModelOutputPart::Text(
                r#"{"action":{"tool":"workspace_write_file","arguments":{"relative_path":"report.txt","content":"private-content"}}}"#.to_owned(),
            )],
            None,
        ))])),
        tool.clone(),
        store.clone(),
        Arc::new(CountingSeal::new()),
        WorkspaceBindingId::new(),
        Arc::new(InMemoryAuditSink::new()),
    );
    let run_id = RunId::new();
    let session_id = SessionId::new();
    let mut context = RunContext::new_graph(
        run_id,
        session_id,
        RunBudget::new(1, 1, 0, Duration::from_secs(30))
            .expect("budget")
            .with_max_approval_requests(1)
            .with_max_graph_steps(3)
            .expect("steps"),
        DurableProgram::RECOVERY_VERSION,
        definition.digest(),
    );
    harness.start_run(&mut context).await.expect("start");
    let result = crate::GraphEngine::new(&definition)
        .run(
            &harness,
            &mut context,
            &mut DurableProgram {
                action_callbacks: Arc::new(AtomicUsize::new(0)),
            },
            &mut DurableState::default(),
        )
        .await;
    assert!(result.is_err());
    assert_eq!(tool.invocation_count(), 0);

    let key = RunKey::new(run_id, session_id);
    assert!(
        store
            .load_pending_approval_requests(key)
            .await
            .expect("pending waits")
            .is_empty()
    );
    let loaded = store.load_run(key).await.expect("durable run");
    assert!(!loaded.events().iter().any(|event| matches!(
        event.kind(),
        AgentEventKind::Graph {
            event: agent_core::GraphProgressEvent::GraphSuspended { .. }
        }
    )));
}

#[tokio::test]
async fn durable_waiting_abort_is_terminal_and_rejects_stale_approval() {
    let definition = durable_definition();
    let directory = tempfile::tempdir().expect("directory");
    let store = Arc::new(
        agent_persistence_sqlite::SqliteRunPersistence::open(
            directory.path().join("abort.sqlite3"),
        )
        .await
        .expect("store"),
    );
    let sealer: Arc<dyn ActionSealPort> = Arc::new(CountingSeal::new());
    let workspace = WorkspaceBindingId::new();
    let tool = Arc::new(DurablePreviewTool::new(durable_tool_definition()));
    let model = Arc::new(FakeModelPort::scripted(vec![Ok(ModelResponse::new(
        vec![ModelOutputPart::Text(
            r#"{"action":{"tool":"workspace_write_file","arguments":{"relative_path":"abort.txt","content":"never-written"}}}"#.to_owned(),
        )],
        None,
    ))]));
    let harness = durable_harness(
        model,
        tool.clone(),
        store.clone(),
        sealer,
        workspace,
        Arc::new(InMemoryAuditSink::new()),
    );
    let key = RunKey::new(RunId::new(), SessionId::new());
    let mut context = RunContext::new_graph(
        key.run_id(),
        key.session_id(),
        RunBudget::new(1, 1, 0, Duration::from_secs(30))
            .expect("budget")
            .with_max_approval_requests(1)
            .with_max_graph_steps(4)
            .expect("steps"),
        DurableProgram::RECOVERY_VERSION,
        definition.digest(),
    );
    harness.start_run(&mut context).await.expect("start");
    let waiting = crate::GraphEngine::new(&definition)
        .run(
            &harness,
            &mut context,
            &mut DurableProgram {
                action_callbacks: Arc::new(AtomicUsize::new(0)),
            },
            &mut DurableState::default(),
        )
        .await
        .expect("waiting");
    let wait = waiting.waiting().expect("wait");
    let waiting_page = store.list_waiting(None, 8).await.expect("waiting page");
    assert_eq!(waiting_page.items().len(), 1);
    assert_eq!(waiting_page.items()[0].wait_id(), wait.wait_id());
    harness
        .abort_durable_waiting(key, wait.wait_id(), 0)
        .await
        .expect("abort");

    assert!(matches!(
        harness.recover_run(key).await.expect("recover"),
        RecoveryDisposition::TerminalFailure {
            outcome: RunOutcome::Cancelled,
            ..
        }
    ));
    assert_eq!(tool.invocation_count(), 0);
    assert!(
        store
            .list_waiting(None, 8)
            .await
            .expect("waiting page")
            .items()
            .is_empty()
    );
    assert!(
        store
            .load_pending_approval_requests(key)
            .await
            .expect("waits")
            .is_empty()
    );
    assert!(
        harness
            .record_durable_approval_outcome(
                key,
                wait.wait_id(),
                0,
                DurableApprovalOutcome::Approve,
            )
            .await
            .is_err()
    );
}
