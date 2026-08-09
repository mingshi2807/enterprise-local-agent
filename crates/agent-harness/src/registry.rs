use std::{collections::HashMap, sync::Arc};

use agent_core::ToolName;
use thiserror::Error;

use crate::ToolPort;

#[derive(Default)]
pub struct ToolRegistry {
    tools: HashMap<ToolName, Arc<dyn ToolPort>>,
}

impl ToolRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, tool: Arc<dyn ToolPort>) -> Result<(), ToolRegistryError> {
        let name = tool.definition().name().clone();
        if self.tools.contains_key(&name) {
            return Err(ToolRegistryError::DuplicateName { name });
        }
        self.tools.insert(name, tool);
        Ok(())
    }

    #[must_use]
    pub fn get(&self, name: &ToolName) -> Option<Arc<dyn ToolPort>> {
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
}
