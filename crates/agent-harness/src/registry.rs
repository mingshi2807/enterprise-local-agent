use std::{collections::HashMap, sync::Arc};

use agent_core::{ToolDefinition, ToolName};
use thiserror::Error;

use crate::ToolPort;

#[derive(Default)]
pub struct ToolRegistry {
    tools: HashMap<ToolName, ToolBinding>,
}

#[derive(Clone)]
pub struct ToolBinding {
    definition: ToolDefinition,
    port: Arc<dyn ToolPort>,
}

impl ToolBinding {
    #[must_use]
    pub const fn definition(&self) -> &ToolDefinition {
        &self.definition
    }

    #[must_use]
    pub fn port(&self) -> Arc<dyn ToolPort> {
        Arc::clone(&self.port)
    }
}

impl ToolRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, tool: Arc<dyn ToolPort>) -> Result<(), ToolRegistryError> {
        let definition = tool.definition().clone();
        let name = definition.name().clone();
        if self.tools.contains_key(&name) {
            return Err(ToolRegistryError::DuplicateName { name });
        }
        self.tools.insert(
            name,
            ToolBinding {
                definition,
                port: tool,
            },
        );
        Ok(())
    }

    #[must_use]
    pub fn get(&self, name: &ToolName) -> Option<ToolBinding> {
        self.tools.get(name).cloned()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.tools.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }
}

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum ToolRegistryError {
    #[error("tool '{name}' is already registered")]
    DuplicateName { name: ToolName },
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_core::{CapabilityKind, ToolCall, ToolDefinition, ToolResult, ToolSchema};

    use crate::{
        AuthorizationDecision, CapabilityPolicy, M0ReadOnlyPolicy, PortFuture, ToolPortError,
    };

    struct FakeTool {
        definition: ToolDefinition,
    }

    impl FakeTool {
        fn new(name: &str, capability: CapabilityKind) -> Self {
            let definition = ToolDefinition::new(
                agent_core::ToolName::new(name).expect("name must be valid"),
                "test tool",
                capability,
                ToolSchema::new(serde_json::json!({"type": "object"}))
                    .expect("schema must be valid"),
            )
            .expect("definition must be valid");
            Self { definition }
        }
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

    #[test]
    fn registry_enforces_uniqueness_but_not_authorization() {
        let mut registry = ToolRegistry::new();
        let privileged: Arc<dyn ToolPort> =
            Arc::new(FakeTool::new("admin_lookup", CapabilityKind::Privileged));

        registry
            .register(Arc::clone(&privileged))
            .expect("registry must accept valid metadata regardless of policy");
        let duplicate = registry
            .register(privileged)
            .expect_err("duplicate name must be rejected");

        assert!(matches!(duplicate, ToolRegistryError::DuplicateName { .. }));
        let registered = registry
            .get(&agent_core::ToolName::new("admin_lookup").expect("name must be valid"))
            .expect("tool must be registered");
        assert_eq!(
            M0ReadOnlyPolicy.authorize(registered.definition().capability()),
            AuthorizationDecision::Denied(crate::PolicyDenial::new(
                CapabilityKind::Privileged,
                crate::PolicyDenialReason::CapabilityNotAllowedInM0,
            ))
        );
    }

    #[test]
    fn registry_binding_uses_the_port_and_its_own_definition() {
        let mut registry = ToolRegistry::new();
        let concrete = Arc::new(FakeTool::new("lookup", CapabilityKind::ReadOnly));
        let expected_definition = concrete.definition().clone();
        let port: Arc<dyn ToolPort> = concrete;

        registry
            .register(Arc::clone(&port))
            .expect("port-derived metadata must register");
        let binding = registry
            .get(expected_definition.name())
            .expect("registered binding must be present");
        let bound_port = binding.port();

        assert_eq!(binding.definition(), &expected_definition);
        assert_eq!(bound_port.definition(), binding.definition());
        assert!(Arc::ptr_eq(&bound_port, &port));
    }
}
