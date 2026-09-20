//! Provider-independent domain values for the enterprise agent runtime.

mod action;
mod budget;
mod capability;
mod event;
mod graph_control;
mod ids;
mod loop_control;
mod model;
mod run;
mod tool;

pub use action::{ActionDigest, ActionProposal, ActionRejectionReason, ToolContractDigest};
pub use budget::{BudgetDimension, BudgetError, BudgetUsage, RunBudget};
pub use capability::CapabilityKind;
pub use event::{
    AgentEvent, AgentEventKind, ApprovalFailureKind, CURRENT_EVENT_SCHEMA_VERSION,
    ContainmentFailureKind, DurableApprovalOutcome, EventSchemaVersion, EventSequence,
    KnowledgeBackendId, KnowledgeEvidenceReference, KnowledgeFailureKind, KnowledgeRouteMetadata,
    KnowledgeSnapshotMetadata, MAX_DURABLE_EVIDENCE_REFERENCES, MAX_DURABLE_KNOWLEDGE_BACKENDS,
    MAX_DURABLE_KNOWLEDGE_ID_BYTES,
};
pub use graph_control::{
    GraphBranchId, GraphBranchIdError, GraphFailureKind, GraphNodeId, GraphNodeIdError,
    GraphNodeKind, GraphProgressEvent, GraphRecoveryMode, GraphTransitionKey,
    MAX_GRAPH_BRANCH_ID_BYTES, MAX_GRAPH_NODE_ID_BYTES, MAX_GRAPH_STEPS,
};
pub use ids::{
    ActionProposalId, ApprovalRequestId, DurableApprovalWaitId, GraphNodeAttemptId,
    KnowledgeRetrievalId, MAX_PRINCIPAL_ID_BYTES, ModelCallId, PrincipalId, PrincipalIdError,
    RunId, SessionId, ToolCallId, WorkspaceBindingId,
};
pub use loop_control::{
    LoopDecisionKind, LoopEventKind, LoopFailureKind, LoopPhase, LoopProgressEvent,
};
pub use model::{
    ModelMessage, ModelOutputPart, ModelRequest, ModelResponse, ModelRole, TokenUsage,
};
pub use run::{RunFailureKind, RunOutcome, RunStateError, RunStatus};
pub use tool::{
    ToolCall, ToolDefinition, ToolDefinitionError, ToolDomainFailure, ToolDomainFailureKind,
    ToolInput, ToolName, ToolNameError, ToolOutput, ToolResult, ToolSchema, ToolSchemaError,
};
