use agent_core::{BudgetDimension, RunStatus, ToolName};
use thiserror::Error;

use crate::{
    AuditPortError, BudgetExceeded, ModelPortError, PolicyDenial, RunContextError, ToolPortError,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HarnessOperation {
    StartRun,
    Checkpoint,
    BeginIteration,
    RecordLoopProgress,
    InvokeModel,
    InvokeTool,
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
    #[error("audit infrastructure failed during {phase:?} for {operation:?}")]
    Audit {
        phase: AuditPhase,
        operation: HarnessOperation,
        effect: OperationEffect,
        kind: AuditPortError,
    },
    #[error("model adapter failed")]
    ModelPort(ModelPortError),
    #[error("tool adapter failed")]
    ToolPort(ToolPortError),
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
