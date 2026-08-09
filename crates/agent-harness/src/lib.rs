//! Execution context, policy, registry, and adapter ports for the agent runtime.

mod audit;
mod context;
mod error;
mod execution;
mod policy;
mod ports;
mod registry;

#[cfg(any(test, feature = "test-support"))]
pub mod testing;

pub use audit::{AuditFailurePolicy, HarnessConfig, HarnessConfigError};
pub use context::{BudgetExceeded, RunCancellationHandle, RunContext, RunContextError};
pub use error::{AuditPhase, ExecutionStage, HarnessError, HarnessOperation, OperationEffect};
pub use execution::ExecutionHarness;
pub use policy::{
    AuthorizationDecision, CapabilityPolicy, M0ReadOnlyPolicy, PolicyDenial, PolicyDenialReason,
};
pub use ports::{
    AuditPortError, AuditSink, ModelPort, ModelPortError, PortFuture, ToolPort, ToolPortError,
};
pub use registry::{ToolBinding, ToolRegistry, ToolRegistryError};

#[cfg(test)]
mod execution_tests;
