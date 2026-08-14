use serde::{Deserialize, Serialize};

use crate::{
    CapabilityKind, LoopEventKind, ModelCallId, RunId, RunOutcome, TokenUsage, ToolCallId,
    ToolDomainFailureKind, ToolName,
};

pub const CURRENT_EVENT_SCHEMA_VERSION: EventSchemaVersion = EventSchemaVersion::new(3);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct EventSchemaVersion(u16);

impl EventSchemaVersion {
    #[must_use]
    pub const fn new(value: u16) -> Self {
        Self(value)
    }

    #[must_use]
    pub const fn get(self) -> u16 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct EventSequence(u64);

impl EventSequence {
    #[must_use]
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }

    #[must_use]
    pub const fn checked_next(self) -> Option<Self> {
        match self.0.checked_add(1) {
            Some(value) => Some(Self(value)),
            None => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentEvent {
    schema_version: EventSchemaVersion,
    run_id: RunId,
    sequence: EventSequence,
    kind: AgentEventKind,
}

impl AgentEvent {
    #[must_use]
    pub const fn new(run_id: RunId, sequence: EventSequence, kind: AgentEventKind) -> Self {
        Self {
            schema_version: CURRENT_EVENT_SCHEMA_VERSION,
            run_id,
            sequence,
            kind,
        }
    }

    #[must_use]
    pub const fn schema_version(&self) -> EventSchemaVersion {
        self.schema_version
    }

    #[must_use]
    pub const fn run_id(&self) -> RunId {
        self.run_id
    }

    #[must_use]
    pub const fn sequence(&self) -> EventSequence {
        self.sequence
    }

    #[must_use]
    pub const fn kind(&self) -> &AgentEventKind {
        &self.kind
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentEventKind {
    RunStarted,
    RunFinished {
        outcome: RunOutcome,
    },
    ModelInvocationStarted {
        model_call_id: ModelCallId,
        usage: u32,
        limit: u32,
    },
    ModelInvocationCompleted {
        model_call_id: ModelCallId,
        token_usage: Option<TokenUsage>,
    },
    ModelInvocationFailed {
        model_call_id: ModelCallId,
    },
    ToolInvocationStarted {
        tool_call_id: ToolCallId,
        tool_name: ToolName,
        capability: CapabilityKind,
        usage: u32,
        limit: u32,
    },
    ToolInvocationCompleted {
        tool_call_id: ToolCallId,
    },
    ToolInvocationDomainFailed {
        tool_call_id: ToolCallId,
        kind: ToolDomainFailureKind,
    },
    ToolInvocationAdapterFailed {
        tool_call_id: ToolCallId,
    },
    ToolPolicyDenied {
        tool_call_id: ToolCallId,
        tool_name: ToolName,
        capability: CapabilityKind,
    },
    Loop {
        event: LoopEventKind,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_envelope_is_versioned_and_deterministically_ordered() {
        let event = AgentEvent::new(
            RunId::new(),
            EventSequence::new(7),
            AgentEventKind::RunStarted,
        );

        assert_eq!(event.schema_version(), CURRENT_EVENT_SCHEMA_VERSION);
        assert_eq!(event.schema_version().get(), 3);
        assert_eq!(event.sequence().get(), 7);

        let json = serde_json::to_string(&event).expect("event must serialize");
        assert!(!json.contains("prompt"));
        assert!(!json.contains("input_schema"));
    }

    #[test]
    fn loop_events_are_metadata_only() {
        let event = AgentEvent::new(
            RunId::new(),
            EventSequence::new(2),
            AgentEventKind::Loop {
                event: LoopEventKind::ReflectDecision {
                    iteration: 1,
                    decision: crate::LoopDecisionKind::Complete,
                },
            },
        );

        let json = serde_json::to_string(&event).expect("event must serialize");

        assert!(json.contains("reflect_decision"));
        assert!(json.contains("complete"));
        for sentinel in [
            "sentinel raw prompt",
            "sentinel model output",
            "sentinel tool input",
            "sentinel tool output",
            "provider failed noisily",
            "input_schema",
        ] {
            assert!(!json.contains(sentinel));
        }
    }

    #[test]
    fn model_events_carry_correlation_id_without_payloads() {
        let model_call_id = ModelCallId::new();
        let event = AgentEvent::new(
            RunId::new(),
            EventSequence::new(0),
            AgentEventKind::ModelInvocationCompleted {
                model_call_id,
                token_usage: Some(TokenUsage::new(Some(3), Some(5))),
            },
        );

        let json = serde_json::to_string(&event).expect("event must serialize");

        assert!(json.contains(&model_call_id.to_string()));
        assert!(!json.contains("sentinel prompt"));
        assert!(!json.contains("sentinel model output"));
    }

    #[test]
    fn tool_events_carry_correlation_id_without_payloads() {
        let tool_call_id = ToolCallId::new();
        let tool_name = ToolName::new("lookup").expect("tool name must be valid");
        let event = AgentEvent::new(
            RunId::new(),
            EventSequence::new(0),
            AgentEventKind::ToolInvocationDomainFailed {
                tool_call_id,
                kind: ToolDomainFailureKind::InvalidInput,
            },
        );
        let denied = AgentEvent::new(
            RunId::new(),
            EventSequence::new(1),
            AgentEventKind::ToolPolicyDenied {
                tool_call_id,
                tool_name,
                capability: CapabilityKind::LocalWrite,
            },
        );

        let json = serde_json::to_string(&(event, denied)).expect("events must serialize");

        assert!(json.contains(&tool_call_id.to_string()));
        assert!(!json.contains("sentinel tool input"));
        assert!(!json.contains("sentinel tool output"));
        assert!(!json.contains("provider failed noisily"));
    }
}
