use std::fmt;

use agent_core::{
    ActionDigest, ActionProposalId, ApprovalRequestId, CapabilityKind, ToolCallId, ToolName,
};
use thiserror::Error;

use crate::PortFuture;

const MAX_APPROVAL_PREVIEW_BYTES: usize = 256;

pub struct ApprovalPreview {
    summary: String,
    target_label: String,
}

impl ApprovalPreview {
    pub fn new(
        summary: impl Into<String>,
        target_label: impl Into<String>,
    ) -> Result<Self, ApprovalPreviewError> {
        let summary = validate_preview_part(summary.into())?;
        let target_label = validate_preview_part(target_label.into())?;
        Ok(Self {
            summary,
            target_label,
        })
    }

    #[must_use]
    pub fn summary(&self) -> &str {
        &self.summary
    }

    #[must_use]
    pub fn target_label(&self) -> &str {
        &self.target_label
    }
}

impl fmt::Debug for ApprovalPreview {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ApprovalPreview")
            .field("summary", &"[REDACTED]")
            .field("target_label", &"[REDACTED]")
            .finish()
    }
}

fn validate_preview_part(value: String) -> Result<String, ApprovalPreviewError> {
    let value = value.trim().to_owned();
    if value.is_empty() {
        return Err(ApprovalPreviewError::Empty);
    }
    if value.len() > MAX_APPROVAL_PREVIEW_BYTES {
        return Err(ApprovalPreviewError::TooLarge);
    }
    if value.chars().any(char::is_control) {
        return Err(ApprovalPreviewError::ControlCharacter);
    }
    Ok(value)
}

#[derive(Clone, Copy, Debug, Error, PartialEq, Eq)]
pub enum ApprovalPreviewError {
    #[error("approval preview must not be empty")]
    Empty,
    #[error("approval preview exceeds the M6 size limit")]
    TooLarge,
    #[error("approval preview must not contain control characters")]
    ControlCharacter,
}

pub struct ApprovalRequest {
    id: ApprovalRequestId,
    action_proposal_id: ActionProposalId,
    tool_call_id: ToolCallId,
    tool_name: ToolName,
    capability: CapabilityKind,
    action_digest: ActionDigest,
    preview: ApprovalPreview,
}

impl ApprovalRequest {
    pub(crate) const fn new(
        id: ApprovalRequestId,
        action_proposal_id: ActionProposalId,
        tool_call_id: ToolCallId,
        tool_name: ToolName,
        capability: CapabilityKind,
        action_digest: ActionDigest,
        preview: ApprovalPreview,
    ) -> Self {
        Self {
            id,
            action_proposal_id,
            tool_call_id,
            tool_name,
            capability,
            action_digest,
            preview,
        }
    }

    #[must_use]
    pub const fn id(&self) -> ApprovalRequestId {
        self.id
    }

    #[must_use]
    pub const fn action_proposal_id(&self) -> ActionProposalId {
        self.action_proposal_id
    }

    #[must_use]
    pub const fn tool_call_id(&self) -> ToolCallId {
        self.tool_call_id
    }

    #[must_use]
    pub const fn tool_name(&self) -> &ToolName {
        &self.tool_name
    }

    #[must_use]
    pub const fn capability(&self) -> CapabilityKind {
        self.capability
    }

    #[must_use]
    pub const fn action_digest(&self) -> ActionDigest {
        self.action_digest
    }

    #[must_use]
    pub const fn preview(&self) -> &ApprovalPreview {
        &self.preview
    }

    #[must_use]
    pub const fn approve(&self) -> ApprovalDecision {
        ApprovalDecision::approved(self.id, self.action_digest)
    }

    #[must_use]
    pub const fn deny(&self) -> ApprovalDecision {
        ApprovalDecision::denied(self.id, self.action_digest)
    }
}

impl fmt::Debug for ApprovalRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ApprovalRequest")
            .field("id", &self.id)
            .field("action_proposal_id", &self.action_proposal_id)
            .field("tool_call_id", &self.tool_call_id)
            .field("tool_name", &self.tool_name)
            .field("capability", &self.capability)
            .field("action_digest", &self.action_digest)
            .field("preview", &self.preview)
            .finish()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ApprovalOutcome {
    Granted,
    Denied,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ApprovalDecision {
    request_id: ApprovalRequestId,
    action_digest: ActionDigest,
    outcome: ApprovalOutcome,
}

impl ApprovalDecision {
    #[must_use]
    pub const fn approved(request_id: ApprovalRequestId, action_digest: ActionDigest) -> Self {
        Self {
            request_id,
            action_digest,
            outcome: ApprovalOutcome::Granted,
        }
    }

    #[must_use]
    pub const fn denied(request_id: ApprovalRequestId, action_digest: ActionDigest) -> Self {
        Self {
            request_id,
            action_digest,
            outcome: ApprovalOutcome::Denied,
        }
    }

    #[must_use]
    pub const fn request_id(self) -> ApprovalRequestId {
        self.request_id
    }

    #[must_use]
    pub const fn action_digest(self) -> ActionDigest {
        self.action_digest
    }

    #[must_use]
    pub const fn outcome(self) -> ApprovalOutcome {
        self.outcome
    }
}

pub trait ApprovalPort: Send + Sync {
    fn decide<'a>(
        &'a self,
        request: &'a ApprovalRequest,
    ) -> PortFuture<'a, Result<ApprovalDecision, ApprovalPortError>>;
}

#[derive(Clone, Copy, Debug, Error, PartialEq, Eq)]
pub enum ApprovalPortError {
    #[error("approval port is unavailable")]
    Unavailable,
    #[error("approval port failed")]
    Failed,
}
