use std::fmt;

use agent_core::{
    ActionDigest, ActionProposalId, ApprovalRequestId, DurableApprovalOutcome,
    DurableApprovalWaitId, GraphNodeAttemptId, GraphNodeId, PrincipalId, RunId, SessionId,
    ToolCallId, ToolContractDigest, WorkspaceBindingId,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{ApprovalPreview, RunContext, RunKey};
use agent_core::ToolResult;

pub const LOCAL_WRITE_CAPSULE_VERSION: u16 = 1;
pub const MAX_DURABLE_RELATIVE_PATH_BYTES: usize = 240;
pub const MAX_DURABLE_CONTENT_BYTES: usize = 4 * 1024;
pub const MAX_SEALED_CAPSULE_BYTES: usize = 8 * 1024;
pub const MAX_SEAL_KEY_ID_BYTES: usize = 64;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct GraphDefinitionDigest([u8; 32]);

impl GraphDefinitionDigest {
    #[must_use]
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    #[must_use]
    pub const fn as_bytes(self) -> [u8; 32] {
        self.0
    }
}

impl fmt::Debug for GraphDefinitionDigest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("GraphDefinitionDigest([REDACTED])")
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocalWriteActionCapsuleV1 {
    relative_path: String,
    content: String,
}

impl LocalWriteActionCapsuleV1 {
    pub fn new(relative_path: String, content: String) -> Result<Self, ActionSealError> {
        if relative_path.is_empty()
            || relative_path.len() > MAX_DURABLE_RELATIVE_PATH_BYTES
            || relative_path.starts_with('/')
            || relative_path.chars().any(char::is_control)
            || relative_path
                .split('/')
                .any(|part| part.is_empty() || part == "." || part == "..")
            || content.len() > MAX_DURABLE_CONTENT_BYTES
        {
            return Err(ActionSealError::InvalidCapsule);
        }
        Ok(Self {
            relative_path,
            content,
        })
    }

    #[must_use]
    pub fn relative_path(&self) -> &str {
        &self.relative_path
    }

    #[must_use]
    pub fn content(&self) -> &str {
        &self.content
    }
}

impl fmt::Debug for LocalWriteActionCapsuleV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LocalWriteActionCapsuleV1")
            .field("relative_path", &"[REDACTED]")
            .field("content", &"[REDACTED]")
            .field("content_bytes", &self.content.len())
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActionSealBinding {
    run_id: RunId,
    session_id: SessionId,
    program_version: u32,
    graph_digest: GraphDefinitionDigest,
    node_id: GraphNodeId,
    attempt_id: GraphNodeAttemptId,
    wait_id: DurableApprovalWaitId,
    approval_request_id: ApprovalRequestId,
    action_proposal_id: ActionProposalId,
    tool_call_id: ToolCallId,
    action_digest: ActionDigest,
    workspace_binding_id: WorkspaceBindingId,
    tool_contract_digest: ToolContractDigest,
    #[serde(default)]
    requester: Option<PrincipalId>,
    capsule_version: u16,
}

impl ActionSealBinding {
    #[allow(clippy::too_many_arguments)]
    pub const fn new(
        key: RunKey,
        program_version: u32,
        graph_digest: GraphDefinitionDigest,
        node_id: GraphNodeId,
        attempt_id: GraphNodeAttemptId,
        wait_id: DurableApprovalWaitId,
        approval_request_id: ApprovalRequestId,
        action_proposal_id: ActionProposalId,
        tool_call_id: ToolCallId,
        action_digest: ActionDigest,
        workspace_binding_id: WorkspaceBindingId,
        tool_contract_digest: ToolContractDigest,
    ) -> Self {
        Self {
            run_id: key.run_id(),
            session_id: key.session_id(),
            program_version,
            graph_digest,
            node_id,
            attempt_id,
            wait_id,
            approval_request_id,
            action_proposal_id,
            tool_call_id,
            action_digest,
            workspace_binding_id,
            tool_contract_digest,
            requester: None,
            capsule_version: LOCAL_WRITE_CAPSULE_VERSION,
        }
    }

    #[must_use]
    pub fn with_requester(mut self, requester: PrincipalId) -> Self {
        self.requester = Some(requester);
        self
    }

    #[must_use]
    pub const fn key(&self) -> RunKey {
        RunKey::new(self.run_id, self.session_id)
    }

    #[must_use]
    pub const fn program_version(&self) -> u32 {
        self.program_version
    }
    #[must_use]
    pub const fn graph_digest(&self) -> GraphDefinitionDigest {
        self.graph_digest
    }
    #[must_use]
    pub const fn node_id(&self) -> &GraphNodeId {
        &self.node_id
    }
    #[must_use]
    pub const fn attempt_id(&self) -> GraphNodeAttemptId {
        self.attempt_id
    }
    #[must_use]
    pub const fn wait_id(&self) -> DurableApprovalWaitId {
        self.wait_id
    }
    #[must_use]
    pub const fn approval_request_id(&self) -> ApprovalRequestId {
        self.approval_request_id
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
    pub const fn action_digest(&self) -> ActionDigest {
        self.action_digest
    }
    #[must_use]
    pub const fn workspace_binding_id(&self) -> WorkspaceBindingId {
        self.workspace_binding_id
    }
    #[must_use]
    pub const fn tool_contract_digest(&self) -> ToolContractDigest {
        self.tool_contract_digest
    }
    #[must_use]
    pub const fn requester(&self) -> Option<&PrincipalId> {
        self.requester.as_ref()
    }
    #[must_use]
    pub const fn capsule_version(&self) -> u16 {
        self.capsule_version
    }
}

impl fmt::Debug for ActionSealBinding {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ActionSealBinding")
            .field("run_id", &self.run_id)
            .field("wait_id", &self.wait_id)
            .field("approval_request_id", &self.approval_request_id)
            .field("action_digest", &"[REDACTED]")
            .field("tool_contract_digest", &"[REDACTED]")
            .finish_non_exhaustive()
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SealedLocalWriteAction {
    version: u16,
    key_id: String,
    nonce: Vec<u8>,
    ciphertext: Vec<u8>,
}

impl SealedLocalWriteAction {
    pub fn new(
        key_id: String,
        nonce: Vec<u8>,
        ciphertext: Vec<u8>,
    ) -> Result<Self, ActionSealError> {
        if key_id.is_empty()
            || key_id.len() > MAX_SEAL_KEY_ID_BYTES
            || key_id
                .chars()
                .any(|c| !c.is_ascii_alphanumeric() && c != '-' && c != '_')
            || nonce.is_empty()
            || ciphertext.is_empty()
            || ciphertext.len() > MAX_SEALED_CAPSULE_BYTES
        {
            return Err(ActionSealError::InvalidCapsule);
        }
        Ok(Self {
            version: LOCAL_WRITE_CAPSULE_VERSION,
            key_id,
            nonce,
            ciphertext,
        })
    }

    #[must_use]
    pub const fn version(&self) -> u16 {
        self.version
    }
    #[must_use]
    pub fn key_id(&self) -> &str {
        &self.key_id
    }
    #[must_use]
    pub fn nonce(&self) -> &[u8] {
        &self.nonce
    }
    #[must_use]
    pub fn ciphertext(&self) -> &[u8] {
        &self.ciphertext
    }

    pub fn validate(&self) -> Result<(), ActionSealError> {
        if self.version != LOCAL_WRITE_CAPSULE_VERSION
            || self.key_id.is_empty()
            || self.key_id.len() > MAX_SEAL_KEY_ID_BYTES
            || self
                .key_id
                .chars()
                .any(|c| !c.is_ascii_alphanumeric() && c != '-' && c != '_')
            || self.nonce.is_empty()
            || self.ciphertext.is_empty()
            || self.ciphertext.len() > MAX_SEALED_CAPSULE_BYTES
        {
            return Err(ActionSealError::InvalidCapsule);
        }
        Ok(())
    }
}

impl fmt::Debug for SealedLocalWriteAction {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SealedLocalWriteAction")
            .field("version", &self.version)
            .field("key_id", &self.key_id)
            .field("nonce", &"[REDACTED]")
            .field("ciphertext", &"[REDACTED]")
            .field("ciphertext_bytes", &self.ciphertext.len())
            .finish()
    }
}

pub trait ActionSealPort: Send + Sync {
    fn seal_local_write(
        &self,
        binding: &ActionSealBinding,
        action: &LocalWriteActionCapsuleV1,
    ) -> Result<SealedLocalWriteAction, ActionSealError>;

    fn open_local_write(
        &self,
        binding: &ActionSealBinding,
        sealed: &SealedLocalWriteAction,
    ) -> Result<LocalWriteActionCapsuleV1, ActionSealError>;
}

#[derive(Clone, Copy, Debug, Error, PartialEq, Eq)]
pub enum ActionSealError {
    #[error("durable action capsule is invalid")]
    InvalidCapsule,
    #[error("durable action capsule key is unavailable")]
    KeyUnavailable,
    #[error("durable action capsule authentication failed")]
    AuthenticationFailed,
    #[error("durable action capsule encoding failed")]
    EncodingFailed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DurableApprovalStatus {
    Waiting,
    DecisionRecordedApprove,
    DecisionRecordedDeny,
    ApprovedReady,
    Executing,
    Consumed,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DurableApprovalRecord {
    binding: ActionSealBinding,
    sealed_action: SealedLocalWriteAction,
    status: DurableApprovalStatus,
    row_version: u64,
    #[serde(default)]
    decision_actor: Option<PrincipalId>,
    #[serde(default)]
    decision_timestamp_unix_millis: Option<u64>,
}

impl DurableApprovalRecord {
    #[must_use]
    pub const fn new(binding: ActionSealBinding, sealed_action: SealedLocalWriteAction) -> Self {
        Self {
            binding,
            sealed_action,
            status: DurableApprovalStatus::Waiting,
            row_version: 0,
            decision_actor: None,
            decision_timestamp_unix_millis: None,
        }
    }
    #[must_use]
    pub const fn binding(&self) -> &ActionSealBinding {
        &self.binding
    }
    #[must_use]
    pub const fn sealed_action(&self) -> &SealedLocalWriteAction {
        &self.sealed_action
    }
    #[must_use]
    pub const fn status(&self) -> DurableApprovalStatus {
        self.status
    }
    #[must_use]
    pub const fn row_version(&self) -> u64 {
        self.row_version
    }
    #[must_use]
    pub const fn decision_actor(&self) -> Option<&PrincipalId> {
        self.decision_actor.as_ref()
    }
    #[must_use]
    pub const fn decision_timestamp_unix_millis(&self) -> Option<u64> {
        self.decision_timestamp_unix_millis
    }

    pub fn restore_store_state(mut self, status: DurableApprovalStatus, row_version: u64) -> Self {
        self.status = status;
        self.row_version = row_version;
        self
    }

    pub fn restore_decision_identity(
        mut self,
        actor: Option<PrincipalId>,
        timestamp_unix_millis: Option<u64>,
    ) -> Self {
        self.decision_actor = actor;
        self.decision_timestamp_unix_millis = timestamp_unix_millis;
        self
    }
}

impl fmt::Debug for DurableApprovalRecord {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DurableApprovalRecord")
            .field("binding", &self.binding)
            .field("sealed_action", &self.sealed_action)
            .field("status", &self.status)
            .field("row_version", &self.row_version)
            .finish()
    }
}

#[derive(Debug)]
pub struct DurableApprovalView {
    wait_id: DurableApprovalWaitId,
    request_id: ApprovalRequestId,
    row_version: u64,
    preview: ApprovalPreview,
}

impl DurableApprovalView {
    pub(crate) const fn new(
        wait_id: DurableApprovalWaitId,
        request_id: ApprovalRequestId,
        row_version: u64,
        preview: ApprovalPreview,
    ) -> Self {
        Self {
            wait_id,
            request_id,
            row_version,
            preview,
        }
    }
    #[must_use]
    pub const fn wait_id(&self) -> DurableApprovalWaitId {
        self.wait_id
    }
    #[must_use]
    pub const fn request_id(&self) -> ApprovalRequestId {
        self.request_id
    }
    #[must_use]
    pub const fn row_version(&self) -> u64 {
        self.row_version
    }
    #[must_use]
    pub const fn preview(&self) -> &ApprovalPreview {
        &self.preview
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DurableApprovalDecisionCommand {
    pub key: RunKey,
    pub wait_id: DurableApprovalWaitId,
    pub approval_request_id: ApprovalRequestId,
    pub action_proposal_id: ActionProposalId,
    pub tool_call_id: ToolCallId,
    pub action_digest: ActionDigest,
    pub expected_row_version: u64,
    pub outcome: DurableApprovalOutcome,
    pub actor: Option<PrincipalId>,
    pub decided_at_unix_millis: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DurableActionContext {
    pub program_version: u32,
    pub graph_digest: GraphDefinitionDigest,
    pub node_id: GraphNodeId,
    pub attempt_id: GraphNodeAttemptId,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DurableApprovalWait {
    wait_id: DurableApprovalWaitId,
    approval_request_id: ApprovalRequestId,
    action_proposal_id: ActionProposalId,
    tool_call_id: ToolCallId,
    action_digest: ActionDigest,
}

pub enum DurableLocalWriteResume {
    Executed {
        context: RunContext,
        attempt_id: GraphNodeAttemptId,
        node_id: GraphNodeId,
        result: ToolResult,
    },
    Denied {
        context: RunContext,
        attempt_id: GraphNodeAttemptId,
        node_id: GraphNodeId,
    },
}

impl DurableApprovalWait {
    pub(crate) const fn new(binding: &ActionSealBinding) -> Self {
        Self {
            wait_id: binding.wait_id(),
            approval_request_id: binding.approval_request_id(),
            action_proposal_id: binding.action_proposal_id(),
            tool_call_id: binding.tool_call_id(),
            action_digest: binding.action_digest(),
        }
    }
    #[must_use]
    pub const fn wait_id(self) -> DurableApprovalWaitId {
        self.wait_id
    }
    #[must_use]
    pub const fn approval_request_id(self) -> ApprovalRequestId {
        self.approval_request_id
    }
    #[must_use]
    pub const fn action_proposal_id(self) -> ActionProposalId {
        self.action_proposal_id
    }
    #[must_use]
    pub const fn tool_call_id(self) -> ToolCallId {
        self.tool_call_id
    }
    #[must_use]
    pub const fn action_digest(self) -> ActionDigest {
        self.action_digest
    }
}
