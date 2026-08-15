use std::fmt;

use serde::{Deserialize, Serialize};

use crate::{ActionProposalId, ModelCallId, ToolInput, ToolName};

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct ActionDigest([u8; 32]);

impl ActionDigest {
    #[must_use]
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }
}

impl fmt::Debug for ActionDigest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ActionDigest([REDACTED])")
    }
}

/// An untrusted model-proposed action.
///
/// A proposal is neither executable nor authorized. The enterprise harness must
/// validate it before it can be bound to a runtime tool call.
#[derive(Clone, PartialEq, Eq)]
pub struct ActionProposal {
    id: ActionProposalId,
    source_model_call_id: ModelCallId,
    tool_name: ToolName,
    arguments: ToolInput,
}

impl ActionProposal {
    #[must_use]
    pub const fn new(
        id: ActionProposalId,
        source_model_call_id: ModelCallId,
        tool_name: ToolName,
        arguments: ToolInput,
    ) -> Self {
        Self {
            id,
            source_model_call_id,
            tool_name,
            arguments,
        }
    }

    #[must_use]
    pub const fn id(&self) -> ActionProposalId {
        self.id
    }

    #[must_use]
    pub const fn source_model_call_id(&self) -> ModelCallId {
        self.source_model_call_id
    }

    #[must_use]
    pub const fn tool_name(&self) -> &ToolName {
        &self.tool_name
    }

    #[must_use]
    pub const fn arguments(&self) -> &ToolInput {
        &self.arguments
    }

    #[must_use]
    pub fn into_parts(self) -> (ActionProposalId, ModelCallId, ToolName, ToolInput) {
        (
            self.id,
            self.source_model_call_id,
            self.tool_name,
            self.arguments,
        )
    }
}

impl fmt::Debug for ActionProposal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ActionProposal")
            .field("id", &self.id)
            .field("source_model_call_id", &self.source_model_call_id)
            .field("tool_name", &self.tool_name)
            .field("arguments", &"[REDACTED]")
            .finish()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionRejectionReason {
    EmptyOutput,
    UnsupportedOutputShape,
    ResponseTooLarge,
    MalformedEnvelope,
    InvalidToolName,
    SecurityLimitExceeded,
    UnknownTool,
    ArgumentsSchemaMismatch,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn action_proposal_debug_redacts_arguments() {
        let sentinel = "proposal-argument-sentinel";
        let proposal = ActionProposal::new(
            ActionProposalId::new(),
            ModelCallId::new(),
            ToolName::new("lookup").expect("tool name must be valid"),
            ToolInput::new(json!({"secret": sentinel})),
        );

        let rendered = format!("{proposal:?}");

        assert!(!rendered.contains(sentinel));
        assert!(rendered.contains("[REDACTED]"));
    }
}
