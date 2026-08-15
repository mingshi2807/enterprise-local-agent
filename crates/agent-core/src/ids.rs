use std::{fmt, str::FromStr};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

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
    }
}
