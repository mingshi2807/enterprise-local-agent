use std::time::Duration;

use thiserror::Error;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuditFailurePolicy {
    FailClosed,
    FailOpen,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HarnessConfig {
    audit_failure_policy: AuditFailurePolicy,
    audit_timeout: Duration,
}

impl HarnessConfig {
    pub fn new(
        audit_failure_policy: AuditFailurePolicy,
        audit_timeout: Duration,
    ) -> Result<Self, HarnessConfigError> {
        if audit_timeout.is_zero() {
            return Err(HarnessConfigError::ZeroAuditTimeout);
        }

        Ok(Self {
            audit_failure_policy,
            audit_timeout,
        })
    }

    #[must_use]
    pub const fn audit_failure_policy(self) -> AuditFailurePolicy {
        self.audit_failure_policy
    }

    #[must_use]
    pub const fn audit_timeout(self) -> Duration {
        self.audit_timeout
    }
}

#[derive(Clone, Copy, Debug, Error, PartialEq, Eq)]
pub enum HarnessConfigError {
    #[error("audit timeout must be non-zero")]
    ZeroAuditTimeout,
}
