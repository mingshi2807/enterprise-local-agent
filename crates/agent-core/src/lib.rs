//! Provider-independent domain values for the enterprise agent runtime.

mod action;
mod budget;
mod capability;
mod event;
mod ids;
mod loop_control;
mod model;
mod run;
mod tool;

pub use action::{ActionDigest, ActionProposal, ActionRejectionReason};
pub use budget::{BudgetDimension, BudgetError, BudgetUsage, RunBudget};
pub use capability::CapabilityKind;
pub use event::{
    AgentEvent, AgentEventKind, ApprovalFailureKind, CURRENT_EVENT_SCHEMA_VERSION,
    ContainmentFailureKind, EventSchemaVersion, EventSequence, KnowledgeBackendId,
    KnowledgeEvidenceReference, KnowledgeFailureKind, KnowledgeRouteMetadata,
    KnowledgeSnapshotMetadata, MAX_DURABLE_EVIDENCE_REFERENCES, MAX_DURABLE_KNOWLEDGE_BACKENDS,
    MAX_DURABLE_KNOWLEDGE_ID_BYTES,
};
pub use ids::{
    ActionProposalId, ApprovalRequestId, KnowledgeRetrievalId, ModelCallId, RunId, SessionId,
    ToolCallId,
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
