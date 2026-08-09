//! Execution context, policy, registry, and adapter ports for the agent runtime.

mod context;
mod policy;
mod ports;
mod registry;

pub use context::{BudgetExceeded, RunContext, RunContextError};
pub use policy::{
    AuthorizationDecision, CapabilityPolicy, M0ReadOnlyPolicy, PolicyDenial, PolicyDenialReason,
};
pub use ports::{
    AuditPortError, AuditSink, ModelPort, ModelPortError, PortFuture, ToolPort, ToolPortError,
};
pub use registry::{ToolRegistry, ToolRegistryError};
