use std::future::Future;
use std::pin::Pin;

use agent_core::{AgentEvent, BudgetUsage, EventSequence, RunBudget, RunId, RunStatus, SessionId};
use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const CURRENT_STORE_SCHEMA_VERSION: u16 = 1;
pub const CURRENT_CHECKPOINT_SCHEMA_VERSION: u16 = 1;

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
    Restartable { version: u32 },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunRecord {
    store_schema_version: u16,
    checkpoint_schema_version: u16,
    key: RunKey,
    budget: RunBudget,
    recovery_contract: RecoveryContract,
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
        }
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
