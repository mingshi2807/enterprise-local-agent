use std::{fmt, str::FromStr};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const MAX_PRINCIPAL_ID_BYTES: usize = 128;

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct PrincipalId(String);

impl PrincipalId {
    pub fn new(value: impl Into<String>) -> Result<Self, PrincipalIdError> {
        let value = value.into();
        if value.is_empty()
            || value.len() > MAX_PRINCIPAL_ID_BYTES
            || !value.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':' | b'@')
            })
        {
            return Err(PrincipalIdError);
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for PrincipalId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("PrincipalId([REDACTED])")
    }
}

impl fmt::Display for PrincipalId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl FromStr for PrincipalId {
    type Err = PrincipalIdError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value)
    }
}

impl TryFrom<String> for PrincipalId {
    type Error = PrincipalIdError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<PrincipalId> for String {
    fn from(value: PrincipalId) -> Self {
        value.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PrincipalIdError;

impl fmt::Display for PrincipalIdError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("principal identifier is invalid")
    }
}

impl std::error::Error for PrincipalIdError {}

macro_rules! define_id {
    ($name:ident) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(Uuid);

        impl $name {
            #[must_use]
            pub fn new() -> Self {
                Self(Uuid::new_v4())
            }

            #[must_use]
            pub const fn from_uuid(value: Uuid) -> Self {
                Self(value)
            }

            #[must_use]
            pub const fn as_uuid(&self) -> &Uuid {
                &self.0
            }

            #[must_use]
            pub const fn into_uuid(self) -> Uuid {
                self.0
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(formatter)
            }
        }

        impl FromStr for $name {
            type Err = uuid::Error;

            fn from_str(value: &str) -> Result<Self, Self::Err> {
                Uuid::parse_str(value).map(Self)
            }
        }
    };
}

define_id!(RunId);
define_id!(SessionId);
define_id!(ModelCallId);
define_id!(ActionProposalId);
define_id!(ToolCallId);
define_id!(ApprovalRequestId);
define_id!(KnowledgeRetrievalId);
define_id!(GraphNodeAttemptId);
define_id!(DurableApprovalWaitId);
define_id!(WorkspaceBindingId);

#[cfg(test)]
mod tests {
    use super::*;

    const UUID_TEXT: &str = "123e4567-e89b-12d3-a456-426614174000";

    fn uuid_fixture() -> Uuid {
        Uuid::parse_str(UUID_TEXT).expect("the test UUID must be valid")
    }

    #[test]
    fn run_id_uses_the_documented_uuid_string_representation() {
        let run_id = RunId::from_uuid(uuid_fixture());

        let serialized = serde_json::to_string(&run_id).expect("RunId must serialize");

        assert_eq!(serialized, format!("\"{UUID_TEXT}\""));
        assert_eq!(run_id.to_string(), UUID_TEXT);
    }

    #[test]
    fn every_id_type_round_trips_through_json() {
        let uuid = uuid_fixture();

        let run_id = RunId::from_uuid(uuid);
        let session_id = SessionId::from_uuid(uuid);
        let model_call_id = ModelCallId::from_uuid(uuid);
        let action_proposal_id = ActionProposalId::from_uuid(uuid);
        let tool_call_id = ToolCallId::from_uuid(uuid);
        let approval_request_id = ApprovalRequestId::from_uuid(uuid);
        let knowledge_retrieval_id = KnowledgeRetrievalId::from_uuid(uuid);
        let graph_node_attempt_id = GraphNodeAttemptId::from_uuid(uuid);
        let durable_approval_wait_id = DurableApprovalWaitId::from_uuid(uuid);
        let workspace_binding_id = WorkspaceBindingId::from_uuid(uuid);

        assert_eq!(
            serde_json::from_str::<RunId>(
                &serde_json::to_string(&run_id).expect("RunId must serialize")
            )
            .expect("RunId must deserialize"),
            run_id
        );
        assert_eq!(
            serde_json::from_str::<SessionId>(
                &serde_json::to_string(&session_id).expect("SessionId must serialize")
            )
            .expect("SessionId must deserialize"),
            session_id
        );
        assert_eq!(
            serde_json::from_str::<ModelCallId>(
                &serde_json::to_string(&model_call_id).expect("ModelCallId must serialize")
            )
            .expect("ModelCallId must deserialize"),
            model_call_id
        );
        assert_eq!(
            serde_json::from_str::<ActionProposalId>(
                &serde_json::to_string(&action_proposal_id)
                    .expect("ActionProposalId must serialize")
            )
            .expect("ActionProposalId must deserialize"),
            action_proposal_id
        );
        assert_eq!(
            serde_json::from_str::<ToolCallId>(
                &serde_json::to_string(&tool_call_id).expect("ToolCallId must serialize")
            )
            .expect("ToolCallId must deserialize"),
            tool_call_id
        );
        assert_eq!(
            serde_json::from_str::<ApprovalRequestId>(
                &serde_json::to_string(&approval_request_id)
                    .expect("ApprovalRequestId must serialize")
            )
            .expect("ApprovalRequestId must deserialize"),
            approval_request_id
        );
        assert_eq!(
            serde_json::from_str::<KnowledgeRetrievalId>(
                &serde_json::to_string(&knowledge_retrieval_id)
                    .expect("KnowledgeRetrievalId must serialize")
            )
            .expect("KnowledgeRetrievalId must deserialize"),
            knowledge_retrieval_id
        );
        assert_eq!(
            serde_json::from_str::<GraphNodeAttemptId>(
                &serde_json::to_string(&graph_node_attempt_id)
                    .expect("GraphNodeAttemptId must serialize")
            )
            .expect("GraphNodeAttemptId must deserialize"),
            graph_node_attempt_id
        );
        assert_eq!(
            serde_json::from_str::<DurableApprovalWaitId>(
                &serde_json::to_string(&durable_approval_wait_id)
                    .expect("DurableApprovalWaitId must serialize")
            )
            .expect("DurableApprovalWaitId must deserialize"),
            durable_approval_wait_id
        );
        assert_eq!(
            serde_json::from_str::<WorkspaceBindingId>(
                &serde_json::to_string(&workspace_binding_id)
                    .expect("WorkspaceBindingId must serialize")
            )
            .expect("WorkspaceBindingId must deserialize"),
            workspace_binding_id
        );
    }

    #[test]
    fn principal_id_is_bounded_and_redacted() {
        let id = PrincipalId::new("local:engineering-user").expect("valid principal");
        assert_eq!(id.as_str(), "local:engineering-user");
        assert_eq!(format!("{id:?}"), "PrincipalId([REDACTED])");
        assert!(PrincipalId::new("").is_err());
        assert!(PrincipalId::new("contains space").is_err());
        assert!(PrincipalId::new("x".repeat(MAX_PRINCIPAL_ID_BYTES + 1)).is_err());
    }
}
