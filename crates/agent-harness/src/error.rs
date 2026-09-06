use agent_core::{BudgetDimension, CapabilityKind, RunStatus, ToolName};
use thiserror::Error;

use crate::{
    ApprovalPortError, AuditPortError, BudgetExceeded, ContainmentPortError, ModelPortError,
    PersistencePortError, PolicyDenial, RecoveryError, RunContextError, ToolPortError,
};
use agent_knowledge::KnowledgeError;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HarnessOperation {
    StartRun,
    Checkpoint,
    BeginIteration,
    RecordLoopProgress,
    StartGraph,
    EnterGraphNode,
    CompleteGraphNode,
    FinishGraph,
    InvokeModel,
    RetrieveKnowledge,
    BindModelGrounding,
    PrepareAction,
    InvokeValidatedAction,
    InvokeTool,
    RequestApproval,
    CompleteRun,
    FailRun,
    CancelRun,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExecutionStage {
    Preflight,
    PreInvocationAudit,
    Invocation,
    PostInvocationAudit,
    LifecycleAudit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuditPhase {
    BeforeInvocation,
    AfterInvocation,
    Lifecycle,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OperationEffect {
    NotInvoked,
    InvocationStarted,
    StateCommitted,
}

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum HarnessError {
    #[error("operation {operation:?} is invalid while run is {status:?}")]
    InvalidLifecycle {
        operation: HarnessOperation,
        status: RunStatus,
    },
    #[error(transparent)]
    BudgetExceeded(#[from] BudgetExceeded),
    #[error("run was cancelled during {stage:?}")]
    Cancelled { stage: ExecutionStage },
    #[error("run deadline was exhausted during {stage:?}")]
    DeadlineExceeded { stage: ExecutionStage },
    #[error("tool '{name}' is not registered")]
    ToolNotFound { name: ToolName },
    #[error(transparent)]
    PolicyDenied(#[from] PolicyDenial),
    #[error("audit is already degraded and cannot authorize non-read-only execution")]
    AuditDegraded,
    #[error("approval is required but unavailable on this execution path")]
    ApprovalRequired,
    #[error("capability {capability:?} is not executable in M6")]
    CapabilityNotExecutable { capability: CapabilityKind },
    #[error("contained execution is unavailable for tool '{name}'")]
    ContainmentUnavailable { name: ToolName },
    #[error("approval port is not configured")]
    ApprovalPortMissing,
    #[error("approval port failed")]
    ApprovalPort(ApprovalPortError),
    #[error("approval decision did not match the requested action")]
    ApprovalDecisionMismatch,
    #[error("approval decision denied the action")]
    ApprovalDenied,
    #[error("contained executor failed")]
    ContainmentPort(ContainmentPortError),
    #[error("audit infrastructure failed during {phase:?} for {operation:?}")]
    Audit {
        phase: AuditPhase,
        operation: HarnessOperation,
        effect: OperationEffect,
        kind: AuditPortError,
    },
    #[error("model adapter failed")]
    ModelPort(ModelPortError),
    #[error("knowledge adapter failed")]
    KnowledgePort(KnowledgeError),
    #[error("grounded model request does not match the completed retrieval")]
    GroundingMismatch,
    #[error("tool adapter failed")]
    ToolPort(ToolPortError),
    #[error("durable run persistence failed")]
    Persistence(PersistencePortError),
    #[error("durable run recovery failed")]
    Recovery(RecoveryError),
    #[error(transparent)]
    Context(#[from] RunContextError),
}

impl HarnessError {
    #[must_use]
    pub const fn budget_dimension(&self) -> Option<BudgetDimension> {
        match self {
            Self::BudgetExceeded(error) => Some(error.dimension()),
            _ => None,
        }
    }
}
