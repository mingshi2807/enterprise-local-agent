use std::future::Future;
use std::pin::Pin;

use agent_core::{AgentEvent, BudgetUsage, EventSequence, RunBudget, RunId, RunStatus, SessionId};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use agent_core::DurableApprovalWaitId;
use agent_identity::DurableRunAuthorization;

use crate::{DurableApprovalDecisionCommand, DurableApprovalRecord, DurableApprovalStatus};

pub const CURRENT_STORE_SCHEMA_VERSION: u16 = 2;
pub const CURRENT_CHECKPOINT_SCHEMA_VERSION: u16 = 4;

pub type PersistenceFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RunKey {
    run_id: RunId,
    session_id: SessionId,
}

impl RunKey {
    #[must_use]
    pub const fn new(run_id: RunId, session_id: SessionId) -> Self {
        Self { run_id, session_id }
    }

    #[must_use]
    pub const fn run_id(self) -> RunId {
        self.run_id
    }

    #[must_use]
    pub const fn session_id(self) -> SessionId {
        self.session_id
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum RecoveryContract {
    NonRestartable,
    Restartable {
        version: u32,
    },
    RestartableRetrieval {
        version: u32,
    },
    Graph {
        program_version: u32,
        definition_digest: [u8; 32],
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunRecord {
    store_schema_version: u16,
    checkpoint_schema_version: u16,
    key: RunKey,
    budget: RunBudget,
    recovery_contract: RecoveryContract,
    #[serde(default)]
    authorization: Option<DurableRunAuthorization>,
}

impl RunRecord {
    #[must_use]
    pub const fn new(key: RunKey, budget: RunBudget, recovery_contract: RecoveryContract) -> Self {
        Self {
            store_schema_version: CURRENT_STORE_SCHEMA_VERSION,
            checkpoint_schema_version: CURRENT_CHECKPOINT_SCHEMA_VERSION,
            key,
            budget,
            recovery_contract,
            authorization: None,
        }
    }

    #[must_use]
    pub fn with_authorization(mut self, authorization: DurableRunAuthorization) -> Self {
        self.authorization = Some(authorization);
        self
    }

    #[must_use]
    pub const fn store_schema_version(&self) -> u16 {
        self.store_schema_version
    }

    #[must_use]
    pub const fn checkpoint_schema_version(&self) -> u16 {
        self.checkpoint_schema_version
    }

    #[must_use]
    pub const fn key(&self) -> RunKey {
        self.key
    }

    #[must_use]
    pub const fn budget(&self) -> &RunBudget {
        &self.budget
    }

    #[must_use]
    pub const fn recovery_contract(&self) -> RecoveryContract {
        self.recovery_contract
    }

    #[must_use]
    pub const fn authorization(&self) -> Option<&DurableRunAuthorization> {
        self.authorization.as_ref()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DurableCheckpoint {
    checkpoint_schema_version: u16,
    state: crate::recovery::DurableRunState,
}

impl DurableCheckpoint {
    #[must_use]
    pub fn initial(record: &RunRecord) -> Self {
        Self::new(crate::DurableRunState::initial(record))
    }

    #[must_use]
    pub const fn new(state: crate::recovery::DurableRunState) -> Self {
        Self {
            checkpoint_schema_version: CURRENT_CHECKPOINT_SCHEMA_VERSION,
            state,
        }
    }

    #[must_use]
    pub const fn checkpoint_schema_version(&self) -> u16 {
        self.checkpoint_schema_version
    }

    #[must_use]
    pub const fn state(&self) -> &crate::recovery::DurableRunState {
        &self.state
    }

    #[must_use]
    pub fn into_state(self) -> crate::recovery::DurableRunState {
        self.state
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppendTransition {
    pub(crate) key: RunKey,
    pub(crate) expected_sequence: Option<EventSequence>,
    pub(crate) event: AgentEvent,
    pub(crate) checkpoint: Option<DurableCheckpoint>,
}

impl AppendTransition {
    #[must_use]
    pub const fn new(
        key: RunKey,
        expected_sequence: Option<EventSequence>,
        event: AgentEvent,
        checkpoint: Option<DurableCheckpoint>,
    ) -> Self {
        Self {
            key,
            expected_sequence,
            event,
            checkpoint,
        }
    }

    #[must_use]
    pub const fn key(&self) -> RunKey {
        self.key
    }

    #[must_use]
    pub const fn expected_sequence(&self) -> Option<EventSequence> {
        self.expected_sequence
    }

    #[must_use]
    pub const fn event(&self) -> &AgentEvent {
        &self.event
    }

    #[must_use]
    pub const fn checkpoint(&self) -> Option<&DurableCheckpoint> {
        self.checkpoint.as_ref()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoadedRun {
    record: RunRecord,
    checkpoint: DurableCheckpoint,
    events: Vec<AgentEvent>,
}

#[derive(Clone, Debug)]
pub struct CreateDurableApprovalWait {
    transition: AppendTransition,
    record: DurableApprovalRecord,
}

impl CreateDurableApprovalWait {
    #[must_use]
    pub const fn new(transition: AppendTransition, record: DurableApprovalRecord) -> Self {
        Self { transition, record }
    }
    #[must_use]
    pub const fn transition(&self) -> &AppendTransition {
        &self.transition
    }
    #[must_use]
    pub const fn record(&self) -> &DurableApprovalRecord {
        &self.record
    }
}

#[derive(Clone, Debug)]
pub struct RecordDurableApprovalDecision {
    transition: AppendTransition,
    command: DurableApprovalDecisionCommand,
}

impl RecordDurableApprovalDecision {
    #[must_use]
    pub const fn new(
        transition: AppendTransition,
        command: DurableApprovalDecisionCommand,
    ) -> Self {
        Self {
            transition,
            command,
        }
    }
    #[must_use]
    pub const fn transition(&self) -> &AppendTransition {
        &self.transition
    }
    #[must_use]
    pub const fn command(&self) -> &DurableApprovalDecisionCommand {
        &self.command
    }
}

#[derive(Clone, Debug)]
pub struct DurableApprovalStatusTransition {
    transition: Option<AppendTransition>,
    key: RunKey,
    wait_id: DurableApprovalWaitId,
    expected_row_version: u64,
    expected_status: DurableApprovalStatus,
    next_status: DurableApprovalStatus,
}

impl DurableApprovalStatusTransition {
    #[must_use]
    pub const fn new(
        transition: Option<AppendTransition>,
        key: RunKey,
        wait_id: DurableApprovalWaitId,
        expected_row_version: u64,
        expected_status: DurableApprovalStatus,
        next_status: DurableApprovalStatus,
    ) -> Self {
        Self {
            transition,
            key,
            wait_id,
            expected_row_version,
            expected_status,
            next_status,
        }
    }
    #[must_use]
    pub const fn transition(&self) -> Option<&AppendTransition> {
        self.transition.as_ref()
    }
    #[must_use]
    pub const fn key(&self) -> RunKey {
        self.key
    }
    #[must_use]
    pub const fn wait_id(&self) -> DurableApprovalWaitId {
        self.wait_id
    }
    #[must_use]
    pub const fn expected_row_version(&self) -> u64 {
        self.expected_row_version
    }
    #[must_use]
    pub const fn expected_status(&self) -> DurableApprovalStatus {
        self.expected_status
    }
    #[must_use]
    pub const fn next_status(&self) -> DurableApprovalStatus {
        self.next_status
    }
}

impl LoadedRun {
    #[must_use]
    pub const fn new(
        record: RunRecord,
        checkpoint: DurableCheckpoint,
        events: Vec<AgentEvent>,
    ) -> Self {
        Self {
            record,
            checkpoint,
            events,
        }
    }

    #[must_use]
    pub const fn record(&self) -> &RunRecord {
        &self.record
    }

    #[must_use]
    pub const fn checkpoint(&self) -> &DurableCheckpoint {
        &self.checkpoint
    }

    #[must_use]
    pub fn events(&self) -> &[AgentEvent] {
        &self.events
    }
}

pub trait RunPersistencePort: Send + Sync {
    fn create_run<'a>(
        &'a self,
        record: &'a RunRecord,
        initial_checkpoint: &'a DurableCheckpoint,
    ) -> PersistenceFuture<'a, Result<(), PersistencePortError>>;

    fn append_transition<'a>(
        &'a self,
        transition: &'a AppendTransition,
    ) -> PersistenceFuture<'a, Result<(), PersistencePortError>>;

    fn load_run<'a>(
        &'a self,
        key: RunKey,
    ) -> PersistenceFuture<'a, Result<LoadedRun, PersistencePortError>>;

    fn create_durable_approval_wait<'a>(
        &'a self,
        _request: &'a CreateDurableApprovalWait,
    ) -> PersistenceFuture<'a, Result<(), PersistencePortError>> {
        Box::pin(async { Err(PersistencePortError::Unavailable) })
    }

    fn load_durable_approval_wait<'a>(
        &'a self,
        _key: RunKey,
        _wait_id: DurableApprovalWaitId,
    ) -> PersistenceFuture<'a, Result<DurableApprovalRecord, PersistencePortError>> {
        Box::pin(async { Err(PersistencePortError::Unavailable) })
    }

    fn load_pending_approval_requests<'a>(
        &'a self,
        _key: RunKey,
    ) -> PersistenceFuture<'a, Result<Vec<DurableApprovalRecord>, PersistencePortError>> {
        Box::pin(async { Err(PersistencePortError::Unavailable) })
    }

    fn record_durable_approval_decision<'a>(
        &'a self,
        _request: &'a RecordDurableApprovalDecision,
    ) -> PersistenceFuture<'a, Result<(), PersistencePortError>> {
        Box::pin(async { Err(PersistencePortError::Unavailable) })
    }

    fn transition_durable_approval_status<'a>(
        &'a self,
        _request: &'a DurableApprovalStatusTransition,
    ) -> PersistenceFuture<'a, Result<(), PersistencePortError>> {
        Box::pin(async { Err(PersistencePortError::Unavailable) })
    }
}

#[derive(Clone, Copy, Debug, Error, PartialEq, Eq)]
pub enum PersistencePortError {
    #[error("durable run storage is unavailable")]
    Unavailable,
    #[error("durable run storage rejected an inconsistent sequence or identity")]
    Conflict,
    #[error("durable run storage is corrupt")]
    Corrupt,
    #[error("durable run storage uses an unsupported schema")]
    UnsupportedVersion,
    #[error("durable run storage failed")]
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ContextSnapshot {
    pub(crate) key: RunKey,
    pub(crate) budget: RunBudget,
    pub(crate) usage: BudgetUsage,
    pub(crate) status: RunStatus,
    pub(crate) audit_degraded: bool,
    pub(crate) started_at_unix_millis: Option<u64>,
}
