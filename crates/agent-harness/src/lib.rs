//! Execution context, policy, registry, and adapter ports for the agent runtime.

mod action;
mod approval;
mod audit;
mod containment;
mod context;
mod durable_approval;
mod error;
mod execution;
mod persistence;
mod policy;
mod ports;
mod read;
mod recovery;
mod registry;

#[cfg(any(test, feature = "test-support"))]
pub mod testing;

pub use action::{
    ActionPreparationError, ActionValidationError, CompletedModelInvocation,
    MAX_ACTION_ARGUMENT_ARRAY_ELEMENTS, MAX_ACTION_ARGUMENT_BYTES, MAX_ACTION_ARGUMENT_DEPTH,
    MAX_ACTION_ARGUMENT_KEYS, MAX_ACTION_ARGUMENT_NODES, MAX_ACTION_TOOL_NAME_BYTES,
    MAX_PLANNING_RESPONSE_BYTES, MAX_TOOL_SCHEMA_BYTES, MAX_TOOL_SCHEMA_DEPTH,
    MAX_TOOL_SCHEMA_KEYS, ToolSchemaRegistrationError, ValidatedAction,
    compute_tool_contract_digest, validate_tool_schema,
};
pub use approval::{
    ApprovalDecision, ApprovalOutcome, ApprovalPort, ApprovalPortError, ApprovalPreview,
    ApprovalPreviewError, ApprovalRequest,
};
pub use audit::{AuditFailurePolicy, HarnessConfig, HarnessConfigError};
pub use containment::{ContainedInvocation, ContainedToolPort, ContainmentPortError};
pub use context::{BudgetExceeded, RunCancellationHandle, RunContext, RunContextError};
pub use durable_approval::{
    ActionSealBinding, ActionSealError, ActionSealPort, DurableActionContext,
    DurableApprovalDecisionCommand, DurableApprovalRecord, DurableApprovalStatus,
    DurableApprovalView, DurableApprovalWait, DurableLocalWriteResume, GraphDefinitionDigest,
    LOCAL_WRITE_CAPSULE_VERSION, LocalWriteActionCapsuleV1, MAX_DURABLE_CONTENT_BYTES,
    MAX_DURABLE_RELATIVE_PATH_BYTES, MAX_SEAL_KEY_ID_BYTES, MAX_SEALED_CAPSULE_BYTES,
    SealedLocalWriteAction,
};
pub use error::{AuditPhase, ExecutionStage, HarnessError, HarnessOperation, OperationEffect};
pub use execution::{CompletedKnowledgeRetrieval, ExecutionHarness};
pub use persistence::{
    AppendTransition, CURRENT_CHECKPOINT_SCHEMA_VERSION, CURRENT_STORE_SCHEMA_VERSION,
    CreateDurableApprovalWait, DurableApprovalStatusTransition, DurableCheckpoint, LoadedRun,
    PersistenceFuture, PersistencePortError, RecordDurableApprovalDecision, RecoveryContract,
    RunKey, RunPersistencePort, RunRecord,
};
pub use policy::{
    AuthorizationDecision, CapabilityPolicy, M0ReadOnlyPolicy, M6ApprovalPolicy, PolicyDenial,
    PolicyDenialReason,
};
pub use ports::{
    AuditPortError, AuditSink, ManagedToolInvocation, ManagedToolPort, ModelPort, ModelPortError,
    PortFuture, ToolPort, ToolPortError,
};
pub use read::{
    DurableEventPage, DurableRunPage, DurableRunSummary, DurableWaitingPage, DurableWaitingSummary,
    MAX_READ_PAGE_ITEMS, ReadFuture, RunPageCursor, RunReadPort, WaitingPageCursor,
};
pub use recovery::{
    ContinuationState, DurableGraphPosition, DurableGraphState, DurableLoopPosition,
    DurableRunState, GraphRestartAnchor, ManualReconciliationReason, PendingEffect,
    PendingKnowledgeRetrieval, RecoveredRun, RecoveredWaitingRun, RecoveryDisposition,
    RecoveryError, TransitionError,
};
pub use registry::{ToolBinding, ToolRegistry, ToolRegistryError};

#[cfg(test)]
mod action_tests;
#[cfg(test)]
mod execution_tests;
#[cfg(test)]
mod knowledge_tests;
