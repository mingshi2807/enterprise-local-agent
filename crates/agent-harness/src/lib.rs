//! Execution context, policy, registry, and adapter ports for the agent runtime.

mod action;
mod audit;
mod context;
mod error;
mod execution;
mod policy;
mod ports;
mod registry;

#[cfg(any(test, feature = "test-support"))]
pub mod testing;

pub use action::{
    ActionPreparationError, ActionValidationError, CompletedModelInvocation,
    MAX_ACTION_ARGUMENT_ARRAY_ELEMENTS, MAX_ACTION_ARGUMENT_BYTES, MAX_ACTION_ARGUMENT_DEPTH,
    MAX_ACTION_ARGUMENT_KEYS, MAX_ACTION_ARGUMENT_NODES, MAX_ACTION_TOOL_NAME_BYTES,
    MAX_PLANNING_RESPONSE_BYTES, MAX_TOOL_SCHEMA_BYTES, MAX_TOOL_SCHEMA_DEPTH,
    MAX_TOOL_SCHEMA_KEYS, ToolSchemaRegistrationError, ValidatedAction,
};
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
mod action_tests;
#[cfg(test)]
mod execution_tests;
