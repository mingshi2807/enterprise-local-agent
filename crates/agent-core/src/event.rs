use serde::{Deserialize, Serialize};

use crate::{
    ActionDigest, ActionProposalId, ActionRejectionReason, ApprovalRequestId, CapabilityKind,
    DurableApprovalWaitId, GraphProgressEvent, KnowledgeRetrievalId, LoopEventKind, ModelCallId,
    RunId, RunOutcome, TokenUsage, ToolCallId, ToolContractDigest, ToolDomainFailureKind, ToolName,
    WorkspaceBindingId,
};

pub const CURRENT_EVENT_SCHEMA_VERSION: EventSchemaVersion = EventSchemaVersion::new(9);

pub const MAX_DURABLE_KNOWLEDGE_BACKENDS: usize = 2;
pub const MAX_DURABLE_EVIDENCE_REFERENCES: usize = 8;
pub const MAX_DURABLE_KNOWLEDGE_ID_BYTES: usize = 256;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct EventSchemaVersion(u16);

impl EventSchemaVersion {
    #[must_use]
    pub const fn new(value: u16) -> Self {
        Self(value)
    }

    #[must_use]
    pub const fn get(self) -> u16 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct EventSequence(u64);

impl EventSequence {
    #[must_use]
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }

    #[must_use]
    pub const fn checked_next(self) -> Option<Self> {
        match self.0.checked_add(1) {
            Some(value) => Some(Self(value)),
            None => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentEvent {
    schema_version: EventSchemaVersion,
    run_id: RunId,
    sequence: EventSequence,
    kind: AgentEventKind,
}

impl AgentEvent {
    #[must_use]
    pub const fn new(run_id: RunId, sequence: EventSequence, kind: AgentEventKind) -> Self {
        Self {
            schema_version: CURRENT_EVENT_SCHEMA_VERSION,
            run_id,
            sequence,
            kind,
        }
    }

    #[must_use]
    pub const fn schema_version(&self) -> EventSchemaVersion {
        self.schema_version
    }

    #[must_use]
    pub const fn run_id(&self) -> RunId {
        self.run_id
    }

    #[must_use]
    pub const fn sequence(&self) -> EventSequence {
        self.sequence
    }

    #[must_use]
    pub const fn kind(&self) -> &AgentEventKind {
        &self.kind
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentEventKind {
    RunStarted {
        started_at_unix_millis: u64,
    },
    RunFinished {
        outcome: RunOutcome,
    },
    /// The configured audit sink failed and fail-open execution was selected.
    ///
    /// The durable journal records this metadata even though the unavailable
    /// audit sink cannot record it itself.
    AuditDegraded,
    ModelInvocationStarted {
        model_call_id: ModelCallId,
        usage: u32,
        limit: u32,
    },
    ModelInvocationCompleted {
        model_call_id: ModelCallId,
        token_usage: Option<TokenUsage>,
    },
    ModelInvocationFailed {
        model_call_id: ModelCallId,
    },
    ActionProposed {
        model_call_id: ModelCallId,
        action_proposal_id: ActionProposalId,
        tool_name: ToolName,
    },
    ActionValidated {
        action_proposal_id: ActionProposalId,
    },
    ActionRejected {
        model_call_id: ModelCallId,
        action_proposal_id: ActionProposalId,
        reason: ActionRejectionReason,
    },
    /// The proposal was assigned a runtime tool-call identity.
    ///
    /// This event does not mean the action was authorized, budgeted, invoked,
    /// or executed.
    ActionExecutionBound {
        action_proposal_id: ActionProposalId,
        tool_call_id: ToolCallId,
    },
    ToolInvocationStarted {
        tool_call_id: ToolCallId,
        tool_name: ToolName,
        capability: CapabilityKind,
        usage: u32,
        limit: u32,
    },
    ToolInvocationCompleted {
        tool_call_id: ToolCallId,
    },
    ToolInvocationDomainFailed {
        tool_call_id: ToolCallId,
        kind: ToolDomainFailureKind,
    },
    ToolInvocationAdapterFailed {
        tool_call_id: ToolCallId,
    },
    ToolPolicyDenied {
        tool_call_id: ToolCallId,
        tool_name: ToolName,
        capability: CapabilityKind,
    },
    ApprovalRequested {
        approval_request_id: ApprovalRequestId,
        action_proposal_id: ActionProposalId,
        tool_call_id: ToolCallId,
        tool_name: ToolName,
        capability: CapabilityKind,
        usage: u32,
        limit: u32,
    },
    ApprovalGranted {
        approval_request_id: ApprovalRequestId,
    },
    ApprovalDenied {
        approval_request_id: ApprovalRequestId,
    },
    ApprovalFailed {
        approval_request_id: ApprovalRequestId,
        kind: ApprovalFailureKind,
    },
    DurableApprovalPrepared {
        wait_id: DurableApprovalWaitId,
        approval_request_id: ApprovalRequestId,
        action_proposal_id: ActionProposalId,
        tool_call_id: ToolCallId,
        action_digest: ActionDigest,
        workspace_binding_id: WorkspaceBindingId,
        tool_contract_digest: ToolContractDigest,
        usage: u32,
        limit: u32,
    },
    DurableApprovalDecisionRecorded {
        wait_id: DurableApprovalWaitId,
        approval_request_id: ApprovalRequestId,
        outcome: DurableApprovalOutcome,
        row_version: u64,
    },
    DurableApprovalGranted {
        wait_id: DurableApprovalWaitId,
        approval_request_id: ApprovalRequestId,
    },
    DurableApprovalDenied {
        wait_id: DurableApprovalWaitId,
        approval_request_id: ApprovalRequestId,
    },
    ContainmentFailed {
        tool_call_id: ToolCallId,
        tool_name: ToolName,
        capability: CapabilityKind,
        kind: ContainmentFailureKind,
    },
    KnowledgeRetrievalStarted {
        retrieval_id: KnowledgeRetrievalId,
        route: KnowledgeRouteMetadata,
        query_digest: [u8; 32],
        query_bytes: u16,
    },
    KnowledgeRetrievalRestarted {
        previous_retrieval_id: KnowledgeRetrievalId,
        retrieval_id: KnowledgeRetrievalId,
        route: KnowledgeRouteMetadata,
        query_digest: [u8; 32],
        query_bytes: u16,
    },
    KnowledgeRetrievalCompleted {
        retrieval_id: KnowledgeRetrievalId,
        snapshots: Vec<KnowledgeSnapshotMetadata>,
        evidence_references: Vec<KnowledgeEvidenceReference>,
        evidence_count: u8,
        truncated: bool,
        degraded: bool,
        manifest_digest: [u8; 32],
    },
    KnowledgeRetrievalFailed {
        retrieval_id: KnowledgeRetrievalId,
        kind: KnowledgeFailureKind,
    },
    ModelGroundingBound {
        retrieval_id: KnowledgeRetrievalId,
        model_call_id: ModelCallId,
        evidence_count: u8,
        manifest_digest: [u8; 32],
    },
    Loop {
        event: LoopEventKind,
    },
    Graph {
        event: GraphProgressEvent,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DurableApprovalOutcome {
    Approve,
    Deny,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KnowledgeBackendId {
    OcppRagKag,
    StandardsMcp,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind", deny_unknown_fields)]
pub enum KnowledgeRouteMetadata {
    Single {
        backend: KnowledgeBackendId,
    },
    Federated {
        backends: Vec<KnowledgeBackendId>,
        allow_partial: bool,
    },
}

impl KnowledgeRouteMetadata {
    #[must_use]
    pub fn backends(&self) -> &[KnowledgeBackendId] {
        match self {
            Self::Single { backend } => std::slice::from_ref(backend),
            Self::Federated { backends, .. } => backends,
        }
    }

    #[must_use]
    pub const fn allow_partial(&self) -> bool {
        matches!(
            self,
            Self::Federated {
                allow_partial: true,
                ..
            }
        )
    }

    #[must_use]
    pub fn is_valid(&self) -> bool {
        let backends = self.backends();
        !backends.is_empty()
            && backends.len() <= MAX_DURABLE_KNOWLEDGE_BACKENDS
            && backends
                .iter()
                .enumerate()
                .all(|(index, backend)| !backends[..index].contains(backend))
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KnowledgeEvidenceReference {
    backend: KnowledgeBackendId,
    reference_id: String,
}

impl KnowledgeEvidenceReference {
    #[must_use]
    pub fn new(backend: KnowledgeBackendId, reference_id: String) -> Option<Self> {
        (!reference_id.is_empty()
            && reference_id.len() <= MAX_DURABLE_KNOWLEDGE_ID_BYTES
            && !reference_id.chars().any(char::is_control))
        .then_some(Self {
            backend,
            reference_id,
        })
    }

    #[must_use]
    pub const fn backend(&self) -> KnowledgeBackendId {
        self.backend
    }

    #[must_use]
    pub fn reference_id(&self) -> &str {
        &self.reference_id
    }

    #[must_use]
    pub fn is_valid(&self) -> bool {
        Self::new(self.backend, self.reference_id.clone()).is_some()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KnowledgeSnapshotMetadata {
    backend: KnowledgeBackendId,
    version: String,
}

impl KnowledgeSnapshotMetadata {
    #[must_use]
    pub fn new(backend: KnowledgeBackendId, version: String) -> Option<Self> {
        (!version.is_empty()
            && version.len() <= MAX_DURABLE_KNOWLEDGE_ID_BYTES
            && !version.chars().any(char::is_control))
        .then_some(Self { backend, version })
    }

    #[must_use]
    pub const fn backend(&self) -> KnowledgeBackendId {
        self.backend
    }

    #[must_use]
    pub fn version(&self) -> &str {
        &self.version
    }

    #[must_use]
    pub fn is_valid(&self) -> bool {
        Self::new(self.backend, self.version.clone()).is_some()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KnowledgeFailureKind {
    Unavailable,
    Rejected,
    MalformedResponse,
    SnapshotMismatch,
    Failed,
    Cancelled,
    DeadlineExceeded,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalFailureKind {
    PortUnavailable,
    PortFailed,
    DecisionMismatch,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContainmentFailureKind {
    Unavailable,
    PreviewRejected,
    Infrastructure,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_envelope_is_versioned_and_deterministically_ordered() {
        let event = AgentEvent::new(
            RunId::new(),
            EventSequence::new(7),
            AgentEventKind::RunStarted {
                started_at_unix_millis: 1,
            },
        );

        assert_eq!(event.schema_version(), CURRENT_EVENT_SCHEMA_VERSION);
        assert_eq!(event.schema_version().get(), 9);
        assert_eq!(event.sequence().get(), 7);

        let json = serde_json::to_string(&event).expect("event must serialize");
        assert!(!json.contains("prompt"));
        assert!(!json.contains("input_schema"));
    }

    #[test]
    fn loop_events_are_metadata_only() {
        let event = AgentEvent::new(
            RunId::new(),
            EventSequence::new(2),
            AgentEventKind::Loop {
                event: LoopEventKind::ReflectDecision {
                    iteration: 1,
                    decision: crate::LoopDecisionKind::Complete,
                },
            },
        );

        let json = serde_json::to_string(&event).expect("event must serialize");

        assert!(json.contains("reflect_decision"));
        assert!(json.contains("complete"));
        for sentinel in [
            "sentinel raw prompt",
            "sentinel model output",
            "sentinel tool input",
            "sentinel tool output",
            "provider failed noisily",
            "input_schema",
        ] {
            assert!(!json.contains(sentinel));
        }
    }

    #[test]
    fn graph_events_are_metadata_only() {
        let event = AgentEvent::new(
            RunId::new(),
            EventSequence::new(2),
            AgentEventKind::Graph {
                event: GraphProgressEvent::GraphNodeCompleted {
                    attempt_id: crate::GraphNodeAttemptId::new(),
                    node_id: crate::GraphNodeId::new("model").expect("node"),
                    transition: crate::GraphTransitionKey::Succeeded,
                    next_node: crate::GraphNodeId::new("action").expect("node"),
                    next_kind: crate::GraphNodeKind::Action,
                    next_recovery: crate::GraphRecoveryMode::Never,
                },
            },
        );
        let json = serde_json::to_string(&event).expect("serialize");
        for forbidden in [
            "prompt",
            "arguments",
            "evidence_content",
            "model_response",
            "tool_result",
            "validated_action",
        ] {
            assert!(!json.contains(forbidden), "found {forbidden}");
        }
    }

    #[test]
    fn model_events_carry_correlation_id_without_payloads() {
        let model_call_id = ModelCallId::new();
        let event = AgentEvent::new(
            RunId::new(),
            EventSequence::new(0),
            AgentEventKind::ModelInvocationCompleted {
                model_call_id,
                token_usage: Some(TokenUsage::new(Some(3), Some(5))),
            },
        );

        let json = serde_json::to_string(&event).expect("event must serialize");

        assert!(json.contains(&model_call_id.to_string()));
        assert!(!json.contains("sentinel prompt"));
        assert!(!json.contains("sentinel model output"));
    }

    #[test]
    fn tool_events_carry_correlation_id_without_payloads() {
        let tool_call_id = ToolCallId::new();
        let tool_name = ToolName::new("lookup").expect("tool name must be valid");
        let event = AgentEvent::new(
            RunId::new(),
            EventSequence::new(0),
            AgentEventKind::ToolInvocationDomainFailed {
                tool_call_id,
                kind: ToolDomainFailureKind::InvalidInput,
            },
        );
        let denied = AgentEvent::new(
            RunId::new(),
            EventSequence::new(1),
            AgentEventKind::ToolPolicyDenied {
                tool_call_id,
                tool_name,
                capability: CapabilityKind::LocalWrite,
            },
        );

        let json = serde_json::to_string(&(event, denied)).expect("events must serialize");

        assert!(json.contains(&tool_call_id.to_string()));
        assert!(!json.contains("sentinel tool input"));
        assert!(!json.contains("sentinel tool output"));
        assert!(!json.contains("provider failed noisily"));
    }

    #[test]
    fn action_events_carry_only_metadata_correlation() {
        let model_call_id = ModelCallId::new();
        let action_proposal_id = ActionProposalId::new();
        let tool_call_id = ToolCallId::new();
        let tool_name = ToolName::new("lookup").expect("tool name must be valid");
        let events = [
            AgentEvent::new(
                RunId::new(),
                EventSequence::new(0),
                AgentEventKind::ActionProposed {
                    model_call_id,
                    action_proposal_id,
                    tool_name,
                },
            ),
            AgentEvent::new(
                RunId::new(),
                EventSequence::new(1),
                AgentEventKind::ActionValidated { action_proposal_id },
            ),
            AgentEvent::new(
                RunId::new(),
                EventSequence::new(2),
                AgentEventKind::ActionExecutionBound {
                    action_proposal_id,
                    tool_call_id,
                },
            ),
        ];

        let json = serde_json::to_string(&events).expect("events must serialize");

        assert!(json.contains(&model_call_id.to_string()));
        assert!(json.contains(&action_proposal_id.to_string()));
        assert!(json.contains(&tool_call_id.to_string()));
        for sentinel in [
            "sentinel model output",
            "sentinel arguments",
            "sentinel schema",
            "sentinel tool result",
            "sentinel provider",
            "sentinel credential",
        ] {
            assert!(!json.contains(sentinel));
        }
    }

    #[test]
    fn approval_and_containment_events_are_metadata_only() {
        let approval_request_id = ApprovalRequestId::new();
        let action_proposal_id = ActionProposalId::new();
        let tool_call_id = ToolCallId::new();
        let tool_name = ToolName::new("write_note").expect("tool name must be valid");
        let events = [
            AgentEvent::new(
                RunId::new(),
                EventSequence::new(0),
                AgentEventKind::ApprovalRequested {
                    approval_request_id,
                    action_proposal_id,
                    tool_call_id,
                    tool_name: tool_name.clone(),
                    capability: CapabilityKind::LocalWrite,
                    usage: 1,
                    limit: 1,
                },
            ),
            AgentEvent::new(
                RunId::new(),
                EventSequence::new(1),
                AgentEventKind::ApprovalGranted {
                    approval_request_id,
                },
            ),
            AgentEvent::new(
                RunId::new(),
                EventSequence::new(2),
                AgentEventKind::ContainmentFailed {
                    tool_call_id,
                    tool_name,
                    capability: CapabilityKind::LocalWrite,
                    kind: ContainmentFailureKind::Unavailable,
                },
            ),
        ];

        let json = serde_json::to_string(&events).expect("events must serialize");

        assert!(json.contains(&approval_request_id.to_string()));
        assert!(json.contains(&action_proposal_id.to_string()));
        assert!(json.contains(&tool_call_id.to_string()));
        for sentinel in [
            "sentinel model output",
            "sentinel action argument",
            "sentinel approval preview",
            "ActionDigest",
            "validator diagnostics",
            "raw approval error",
        ] {
            assert!(!json.contains(sentinel));
        }
    }
}
