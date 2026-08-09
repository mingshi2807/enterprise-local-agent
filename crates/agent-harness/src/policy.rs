use agent_core::CapabilityKind;
use serde::{Deserialize, Serialize};
use thiserror::Error;

pub trait CapabilityPolicy: Send + Sync {
    fn authorize(&self, capability: CapabilityKind) -> AuthorizationDecision;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct M0ReadOnlyPolicy;

impl CapabilityPolicy for M0ReadOnlyPolicy {
    fn authorize(&self, capability: CapabilityKind) -> AuthorizationDecision {
        match capability {
            CapabilityKind::ReadOnly => AuthorizationDecision::Allowed,
            CapabilityKind::LocalWrite
            | CapabilityKind::ExternalWrite
            | CapabilityKind::Privileged => AuthorizationDecision::Denied(PolicyDenial::new(
                capability,
                PolicyDenialReason::CapabilityNotAllowedInM0,
            )),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthorizationDecision {
    Allowed,
    Denied(PolicyDenial),
}

#[derive(Clone, Copy, Debug, Error, PartialEq, Eq, Serialize, Deserialize)]
#[error("capability {capability:?} is denied: {reason}")]
pub struct PolicyDenial {
    capability: CapabilityKind,
    reason: PolicyDenialReason,
}

impl PolicyDenial {
    #[must_use]
    pub const fn new(capability: CapabilityKind, reason: PolicyDenialReason) -> Self {
        Self { capability, reason }
    }

    #[must_use]
    pub const fn capability(self) -> CapabilityKind {
        self.capability
    }

    #[must_use]
    pub const fn reason(self) -> PolicyDenialReason {
        self.reason
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PolicyDenialReason {
    CapabilityNotAllowedInM0,
}

impl std::fmt::Display for PolicyDenialReason {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::CapabilityNotAllowedInM0 => {
                formatter.write_str("capability is not allowed in M0")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn m0_policy_allows_only_read_only_capabilities() {
        let policy = M0ReadOnlyPolicy;

        assert_eq!(
            policy.authorize(CapabilityKind::ReadOnly),
            AuthorizationDecision::Allowed
        );
        for capability in [
            CapabilityKind::LocalWrite,
            CapabilityKind::ExternalWrite,
            CapabilityKind::Privileged,
        ] {
            assert!(matches!(
                policy.authorize(capability),
                AuthorizationDecision::Denied(PolicyDenial { .. })
            ));
        }
    }
}
