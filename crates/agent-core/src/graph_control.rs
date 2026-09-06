use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, de};
use thiserror::Error;

use crate::{GraphNodeAttemptId, RunFailureKind};

pub const MAX_GRAPH_NODE_ID_BYTES: usize = 64;
pub const MAX_GRAPH_BRANCH_ID_BYTES: usize = 32;
pub const MAX_GRAPH_STEPS: u32 = 64;

macro_rules! bounded_id {
    ($name:ident, $error:ident, $max:ident) => {
        #[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            pub fn new(value: impl Into<String>) -> Result<Self, $error> {
                let value = value.into();
                if value.is_empty()
                    || value.len() > $max
                    || value.chars().any(|character| {
                        !character.is_ascii_alphanumeric()
                            && !matches!(character, '_' | '-' | '.' | ':')
                    })
                {
                    return Err($error);
                }
                Ok(Self(value))
            }

            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(formatter)
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                let value = String::deserialize(deserializer)?;
                Self::new(value).map_err(de::Error::custom)
            }
        }
    };
}

bounded_id!(GraphNodeId, GraphNodeIdError, MAX_GRAPH_NODE_ID_BYTES);
bounded_id!(GraphBranchId, GraphBranchIdError, MAX_GRAPH_BRANCH_ID_BYTES);

#[derive(Clone, Copy, Debug, Error, PartialEq, Eq)]
#[error("graph node ID is invalid")]
pub struct GraphNodeIdError;

#[derive(Clone, Copy, Debug, Error, PartialEq, Eq)]
#[error("graph branch ID is invalid")]
pub struct GraphBranchIdError;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GraphNodeKind {
    Retrieve,
    Model,
    Action,
    Verify,
    Decision,
    Complete,
    Fail,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GraphRecoveryMode {
    Never,
    FreshRetrieval,
    DeterministicBoundary,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "branch")]
pub enum GraphTransitionKey {
    Succeeded,
    VerificationPassed,
    VerificationFailed,
    Branch(GraphBranchId),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GraphFailureKind {
    Callback,
    InvalidTransition,
    InvariantViolation,
    ExplicitTerminal,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind", deny_unknown_fields)]
pub enum GraphProgressEvent {
    GraphStarted {
        definition_digest: [u8; 32],
        start_node: GraphNodeId,
        start_kind: GraphNodeKind,
        start_recovery: GraphRecoveryMode,
    },
    GraphNodeEntered {
        attempt_id: GraphNodeAttemptId,
        node_id: GraphNodeId,
        node_kind: GraphNodeKind,
        recovery: GraphRecoveryMode,
        step: u32,
        limit: u32,
    },
    GraphNodeRestarted {
        previous_attempt_id: Option<GraphNodeAttemptId>,
        attempt_id: GraphNodeAttemptId,
        node_id: GraphNodeId,
        node_kind: GraphNodeKind,
        recovery: GraphRecoveryMode,
        step: u32,
        limit: u32,
    },
    GraphNodeCompleted {
        attempt_id: GraphNodeAttemptId,
        node_id: GraphNodeId,
        transition: GraphTransitionKey,
        next_node: GraphNodeId,
        next_kind: GraphNodeKind,
        next_recovery: GraphRecoveryMode,
    },
    GraphCompleted {
        attempt_id: GraphNodeAttemptId,
        node_id: GraphNodeId,
        steps: u32,
    },
    GraphFailed {
        attempt_id: GraphNodeAttemptId,
        node_id: GraphNodeId,
        steps: u32,
        failure: GraphFailureKind,
        run_failure: RunFailureKind,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn graph_ids_validate_during_construction_and_deserialization() {
        assert!(GraphNodeId::new("retrieve-1").is_ok());
        assert!(GraphNodeId::new("../escape").is_err());
        assert!(GraphNodeId::new("x".repeat(MAX_GRAPH_NODE_ID_BYTES + 1)).is_err());
        assert!(GraphBranchId::new("passed").is_ok());
        assert!(serde_json::from_str::<GraphNodeId>(r#""bad space""#).is_err());
    }
}
