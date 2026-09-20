use std::{future::Future, pin::Pin};

use agent_core::{AgentEvent, DurableApprovalWaitId, EventSequence, RunId};
use agent_identity::DurableRunAuthorization;

use crate::{DurableApprovalStatus, PersistencePortError, RecoveryContract, RunKey};

pub const MAX_READ_PAGE_ITEMS: u16 = 256;

pub type ReadFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RunPageCursor {
    run_id: RunId,
}

impl RunPageCursor {
    #[must_use]
    pub const fn new(run_id: RunId) -> Self {
        Self { run_id }
    }

    #[must_use]
    pub const fn run_id(self) -> RunId {
        self.run_id
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WaitingPageCursor {
    run_id: RunId,
    wait_id: DurableApprovalWaitId,
}

impl WaitingPageCursor {
    #[must_use]
    pub const fn new(run_id: RunId, wait_id: DurableApprovalWaitId) -> Self {
        Self { run_id, wait_id }
    }

    #[must_use]
    pub const fn run_id(self) -> RunId {
        self.run_id
    }

    #[must_use]
    pub const fn wait_id(self) -> DurableApprovalWaitId {
        self.wait_id
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DurableRunSummary {
    key: RunKey,
    recovery_contract: RecoveryContract,
    last_sequence: Option<EventSequence>,
    terminal: bool,
    authorization: Option<DurableRunAuthorization>,
}

impl DurableRunSummary {
    #[must_use]
    pub const fn new(
        key: RunKey,
        recovery_contract: RecoveryContract,
        last_sequence: Option<EventSequence>,
        terminal: bool,
        authorization: Option<DurableRunAuthorization>,
    ) -> Self {
        Self {
            key,
            recovery_contract,
            last_sequence,
            terminal,
            authorization,
        }
    }

    #[must_use]
    pub const fn key(&self) -> RunKey {
        self.key
    }
    #[must_use]
    pub const fn recovery_contract(&self) -> RecoveryContract {
        self.recovery_contract
    }
    #[must_use]
    pub const fn last_sequence(&self) -> Option<EventSequence> {
        self.last_sequence
    }
    #[must_use]
    pub const fn terminal(&self) -> bool {
        self.terminal
    }
    #[must_use]
    pub const fn authorization(&self) -> Option<&DurableRunAuthorization> {
        self.authorization.as_ref()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DurableRunPage {
    items: Vec<DurableRunSummary>,
    next: Option<RunPageCursor>,
}

impl DurableRunPage {
    #[must_use]
    pub const fn new(items: Vec<DurableRunSummary>, next: Option<RunPageCursor>) -> Self {
        Self { items, next }
    }
    #[must_use]
    pub fn items(&self) -> &[DurableRunSummary] {
        &self.items
    }
    #[must_use]
    pub const fn next(&self) -> Option<RunPageCursor> {
        self.next
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DurableWaitingSummary {
    key: RunKey,
    wait_id: DurableApprovalWaitId,
    row_version: u64,
    status: DurableApprovalStatus,
}

impl DurableWaitingSummary {
    #[must_use]
    pub const fn new(
        key: RunKey,
        wait_id: DurableApprovalWaitId,
        row_version: u64,
        status: DurableApprovalStatus,
    ) -> Self {
        Self {
            key,
            wait_id,
            row_version,
            status,
        }
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
    pub const fn row_version(&self) -> u64 {
        self.row_version
    }
    #[must_use]
    pub const fn status(&self) -> DurableApprovalStatus {
        self.status
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DurableWaitingPage {
    items: Vec<DurableWaitingSummary>,
    next: Option<WaitingPageCursor>,
}

impl DurableWaitingPage {
    #[must_use]
    pub const fn new(items: Vec<DurableWaitingSummary>, next: Option<WaitingPageCursor>) -> Self {
        Self { items, next }
    }
    #[must_use]
    pub fn items(&self) -> &[DurableWaitingSummary] {
        &self.items
    }
    #[must_use]
    pub const fn next(&self) -> Option<WaitingPageCursor> {
        self.next
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DurableEventPage {
    events: Vec<AgentEvent>,
    next: Option<EventSequence>,
}

impl DurableEventPage {
    #[must_use]
    pub const fn new(events: Vec<AgentEvent>, next: Option<EventSequence>) -> Self {
        Self { events, next }
    }
    #[must_use]
    pub fn events(&self) -> &[AgentEvent] {
        &self.events
    }
    #[must_use]
    pub const fn next(&self) -> Option<EventSequence> {
        self.next
    }
}

pub trait RunReadPort: Send + Sync {
    fn find_run<'a>(
        &'a self,
        key: RunKey,
    ) -> ReadFuture<'a, Result<Option<DurableRunSummary>, PersistencePortError>>;
    fn list_runs<'a>(
        &'a self,
        after: Option<RunPageCursor>,
        limit: u16,
    ) -> ReadFuture<'a, Result<DurableRunPage, PersistencePortError>>;
    fn read_events<'a>(
        &'a self,
        key: RunKey,
        after: Option<EventSequence>,
        limit: u16,
    ) -> ReadFuture<'a, Result<DurableEventPage, PersistencePortError>>;
    fn list_waiting<'a>(
        &'a self,
        after: Option<WaitingPageCursor>,
        limit: u16,
    ) -> ReadFuture<'a, Result<DurableWaitingPage, PersistencePortError>>;
}
