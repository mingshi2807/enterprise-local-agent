use std::{future::Future, pin::Pin};

use agent_core::{AgentEvent, ModelRequest, ModelResponse, ToolCall, ToolDefinition, ToolResult};
use thiserror::Error;

pub type PortFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

pub trait ModelPort: Send + Sync {
    fn invoke<'a>(
        &'a self,
        request: ModelRequest,
    ) -> PortFuture<'a, Result<ModelResponse, ModelPortError>>;
}

pub trait ToolPort: Send + Sync {
    fn definition(&self) -> &ToolDefinition;

    fn invoke<'a>(&'a self, call: ToolCall) -> PortFuture<'a, Result<ToolResult, ToolPortError>>;
}

/// A tool whose adapter owns external process lifecycle for one invocation.
pub trait ManagedToolPort: Send + Sync {
    fn definition(&self) -> &ToolDefinition;

    fn start_managed(
        &self,
        call: ToolCall,
    ) -> Result<Box<dyn ManagedToolInvocation>, ToolPortError>;
}

pub trait ManagedToolInvocation: Send {
    fn wait<'a>(&'a mut self) -> PortFuture<'a, Result<ToolResult, ToolPortError>>;

    fn terminate_and_reap<'a>(&'a mut self) -> PortFuture<'a, Result<(), ToolPortError>>;
}

pub trait AuditSink: Send + Sync {
    fn record<'a>(&'a self, event: &'a AgentEvent) -> PortFuture<'a, Result<(), AuditPortError>>;
}

#[derive(Clone, Copy, Debug, Error, PartialEq, Eq)]
pub enum ModelPortError {
    #[error("model adapter is unavailable")]
    Unavailable,
    #[error("model adapter rejected the request")]
    Rejected,
    #[error("model adapter failed")]
    Failed,
}

#[derive(Clone, Copy, Debug, Error, PartialEq, Eq)]
pub enum ToolPortError {
    #[error("tool adapter is unavailable")]
    Unavailable,
    #[error("tool adapter failed before producing a domain result")]
    AdapterFailure,
}

#[derive(Clone, Copy, Debug, Error, PartialEq, Eq)]
pub enum AuditPortError {
    #[error("audit sink is unavailable")]
    Unavailable,
    #[error("audit sink failed to record the event")]
    RecordFailed,
    #[error("audit sink did not record the event before its timeout")]
    TimedOut,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_model_port_is_object_safe(_: &dyn ModelPort) {}
    fn assert_tool_port_is_object_safe(_: &dyn ToolPort) {}
    fn assert_managed_tool_port_is_object_safe(_: &dyn ManagedToolPort) {}
    fn assert_audit_sink_is_object_safe(_: &dyn AuditSink) {}

    #[test]
    fn ports_are_object_safe() {
        struct FakeModel;
        impl ModelPort for FakeModel {
            fn invoke<'a>(
                &'a self,
                _request: ModelRequest,
            ) -> PortFuture<'a, Result<ModelResponse, ModelPortError>> {
                Box::pin(std::future::ready(Err(ModelPortError::Unavailable)))
            }
        }

        struct FakeTool {
            definition: ToolDefinition,
        }
        impl ToolPort for FakeTool {
            fn definition(&self) -> &ToolDefinition {
                &self.definition
            }

            fn invoke<'a>(
                &'a self,
                _call: ToolCall,
            ) -> PortFuture<'a, Result<ToolResult, ToolPortError>> {
                Box::pin(std::future::ready(Err(ToolPortError::Unavailable)))
            }
        }

        struct FakeAudit;
        impl AuditSink for FakeAudit {
            fn record<'a>(
                &'a self,
                _event: &'a AgentEvent,
            ) -> PortFuture<'a, Result<(), AuditPortError>> {
                Box::pin(std::future::ready(Ok(())))
            }
        }

        let model = FakeModel;
        let audit = FakeAudit;
        assert_model_port_is_object_safe(&model);
        assert_audit_sink_is_object_safe(&audit);

        let definition = ToolDefinition::new(
            agent_core::ToolName::new("fake").expect("name must be valid"),
            "fake tool",
            agent_core::CapabilityKind::ReadOnly,
            agent_core::ToolSchema::new(serde_json::json!({"type": "object"}))
                .expect("schema must be valid"),
        )
        .expect("definition must be valid");
        let tool = FakeTool { definition };
        assert_tool_port_is_object_safe(&tool);
        struct FakeManaged(ToolDefinition);
        impl ManagedToolPort for FakeManaged {
            fn definition(&self) -> &ToolDefinition {
                &self.0
            }

            fn start_managed(
                &self,
                _call: ToolCall,
            ) -> Result<Box<dyn ManagedToolInvocation>, ToolPortError> {
                Err(ToolPortError::Unavailable)
            }
        }
        assert_managed_tool_port_is_object_safe(&FakeManaged(tool.definition().clone()));
    }
}
