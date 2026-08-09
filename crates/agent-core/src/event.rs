use serde::{Deserialize, Serialize};

use crate::{BudgetDimension, CapabilityKind, RunId, RunOutcome, RunStatus, ToolCallId, ToolName};

pub const CURRENT_EVENT_SCHEMA_VERSION: EventSchemaVersion = EventSchemaVersion::new(1);

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
    RunStatusChanged {
        status: RunStatus,
    },
    RunFinished {
        outcome: RunOutcome,
    },
    ModelCallReserved {
        usage: u32,
        limit: u32,
    },
    ToolCallReserved {
        usage: u32,
        limit: u32,
    },
    BudgetExceeded {
        dimension: BudgetDimension,
    },
    ToolAuthorized {
        call_id: ToolCallId,
        tool_name: ToolName,
        capability: CapabilityKind,
    },
    ToolDenied {
        call_id: ToolCallId,
        tool_name: ToolName,
        capability: CapabilityKind,
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
            AgentEventKind::RunStatusChanged {
                status: RunStatus::Running,
            },
        );

        assert_eq!(event.schema_version(), CURRENT_EVENT_SCHEMA_VERSION);
        assert_eq!(event.sequence().get(), 7);

        let json = serde_json::to_string(&event).expect("event must serialize");
        assert!(!json.contains("prompt"));
        assert!(!json.contains("input_schema"));
    }
}
