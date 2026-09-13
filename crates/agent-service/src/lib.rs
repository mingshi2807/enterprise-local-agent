use std::{
    collections::{HashMap, HashSet},
    fmt,
    future::Future,
    pin::Pin,
    sync::Arc,
};

use agent_core::{AgentEvent, AgentEventKind, DurableApprovalOutcome, RunOutcome};
pub use agent_core::{DurableApprovalWaitId, EventSequence, RunId, SessionId};
use agent_harness::{
    DurableApprovalStatus, DurableApprovalView, DurableRunPage, DurableWaitingPage,
    ExecutionHarness, MAX_READ_PAGE_ITEMS, PersistencePortError, RecoveredRun, RecoveredWaitingRun,
    RecoveryContract, RecoveryDisposition, RunCancellationHandle, RunContext, RunPageCursor,
    RunReadPort,
};
pub use agent_harness::{RunKey, WaitingPageCursor};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;
use tokio::sync::Mutex;
use uuid::Uuid;

pub const MAX_RUN_INPUT_BYTES: usize = 8 * 1024;
pub const MAX_ACTIVE_RUNS: usize = 32;
pub const MAX_SESSIONS: usize = 256;
pub const MAX_COMMAND_QUEUE: usize = 32;
pub const SERVICE_EVENT_VERSION: u16 = 1;

pub type ServiceFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct WorkflowId(String);

impl WorkflowId {
    pub fn new(value: impl Into<String>) -> Result<Self, ServiceError> {
        let value = value.into();
        if value.is_empty()
            || value.len() > 64
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        {
            return Err(ServiceError::InvalidRequest);
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn validate(&self) -> Result<(), ServiceError> {
        Self::new(self.0.clone()).map(|_| ())
    }
}

#[derive(Clone)]
pub struct RunInput(Vec<u8>);

impl RunInput {
    pub fn new(bytes: Vec<u8>) -> Result<Self, ServiceError> {
        if bytes.is_empty() || bytes.len() > MAX_RUN_INPUT_BYTES {
            return Err(ServiceError::InvalidRequest);
        }
        Ok(Self(bytes))
    }

    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.0
    }
}

impl fmt::Debug for RunInput {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RunInput")
            .field("bytes", &self.0.len())
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunDispositionV1 {
    Starting,
    Running,
    Waiting,
    Resumable,
    Completed,
    Failed,
    ManualReconciliationRequired,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunView {
    pub session_id: SessionId,
    pub run_id: RunId,
    pub disposition: RunDispositionV1,
    pub last_sequence: Option<u64>,
    pub outcome: Option<RunOutcomeV1>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunOutcomeV1 {
    Completed,
    Cancelled,
    BudgetExceeded,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WaitingApprovalView {
    pub session_id: SessionId,
    pub run_id: RunId,
    pub wait_id: DurableApprovalWaitId,
    pub row_version: u64,
    pub state: WaitingStateV1,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WaitingStateV1 {
    Waiting,
    Approved,
    Denied,
    Executing,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalDecisionV1 {
    Approve,
    Deny,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ApprovalPreviewV1 {
    pub wait_id: DurableApprovalWaitId,
    pub row_version: u64,
    pub summary: String,
    pub target: String,
}

pub trait ConfiguredWorkflow: Send + Sync {
    fn id(&self) -> &WorkflowId;
    fn recovery_contract(&self) -> RecoveryContract;
    fn new_context(&self, key: RunKey) -> RunContext;
    fn run(
        self: Arc<Self>,
        harness: Arc<ExecutionHarness>,
        context: RunContext,
        input: RunInput,
    ) -> ServiceFuture<'static, Result<(), WorkflowError>>;
    fn resume_waiting(
        self: Arc<Self>,
        harness: Arc<ExecutionHarness>,
        recovered: RecoveredWaitingRun,
        wait_id: DurableApprovalWaitId,
    ) -> ServiceFuture<'static, Result<(), WorkflowError>>;
    fn resume_recovered(
        self: Arc<Self>,
        _harness: Arc<ExecutionHarness>,
        _recovered: RecoveredRun,
    ) -> ServiceFuture<'static, Result<(), WorkflowError>> {
        Box::pin(async { Err(WorkflowError::NotRestartable) })
    }
}

#[derive(Clone, Copy, Debug, Error, PartialEq, Eq)]
pub enum WorkflowError {
    #[error("configured workflow failed")]
    Failed,
    #[error("configured workflow is not restartable")]
    NotRestartable,
}

struct ActiveRun {
    key: RunKey,
    disposition: RunDispositionV1,
    cancellation: Option<RunCancellationHandle>,
}

pub struct AgentService {
    harness: Arc<ExecutionHarness>,
    read: Arc<dyn RunReadPort>,
    workflows: HashMap<WorkflowId, Arc<dyn ConfiguredWorkflow>>,
    recovery_workflows: Vec<(RecoveryContract, WorkflowId)>,
    sessions: Mutex<HashSet<SessionId>>,
    active: Mutex<HashMap<SessionId, ActiveRun>>,
}

impl AgentService {
    pub fn new(
        harness: Arc<ExecutionHarness>,
        read: Arc<dyn RunReadPort>,
        workflows: Vec<Arc<dyn ConfiguredWorkflow>>,
    ) -> Result<Self, ServiceError> {
        let mut by_id = HashMap::new();
        let mut by_recovery = Vec::new();
        for workflow in workflows {
            let id = workflow.id().clone();
            if by_id.insert(id.clone(), workflow.clone()).is_some()
                || by_recovery
                    .iter()
                    .any(|(contract, _)| *contract == workflow.recovery_contract())
            {
                return Err(ServiceError::Configuration);
            }
            by_recovery.push((workflow.recovery_contract(), id));
        }
        if by_id.is_empty() {
            return Err(ServiceError::Configuration);
        }
        Ok(Self {
            harness,
            read,
            workflows: by_id,
            recovery_workflows: by_recovery,
            sessions: Mutex::new(HashSet::new()),
            active: Mutex::new(HashMap::new()),
        })
    }

    pub async fn create_session(&self) -> Result<SessionId, ServiceError> {
        let mut sessions = self.sessions.lock().await;
        if sessions.len() >= MAX_SESSIONS {
            return Err(ServiceError::Capacity);
        }
        let id = SessionId::new();
        sessions.insert(id);
        Ok(id)
    }

    pub async fn start_run(
        self: &Arc<Self>,
        session_id: SessionId,
        start_request_id: Uuid,
        workflow_id: &WorkflowId,
        input: RunInput,
    ) -> Result<RunView, ServiceError> {
        workflow_id.validate()?;
        if !self.sessions.lock().await.contains(&session_id) {
            return Err(ServiceError::NotFound);
        }
        let workflow = self
            .workflows
            .get(workflow_id)
            .cloned()
            .ok_or(ServiceError::WorkflowUnavailable)?;
        let key = RunKey::new(derive_run_id(session_id, start_request_id), session_id);
        {
            let mut active = self.active.lock().await;
            if let Some(existing) = active.get(&session_id) {
                if existing.key == key {
                    return Ok(active_view(existing));
                }
                return Err(ServiceError::Conflict);
            }
            if active.len() >= MAX_ACTIVE_RUNS {
                return Err(ServiceError::Capacity);
            }
            active.insert(
                session_id,
                ActiveRun {
                    key,
                    disposition: RunDispositionV1::Starting,
                    cancellation: None,
                },
            );
        }

        match self.read.find_run(key).await {
            Ok(Some(_)) => {
                let result = self
                    .harness
                    .recover_run(key)
                    .await
                    .map_err(ServiceError::from)
                    .and_then(disposition_view);
                self.active.lock().await.remove(&session_id);
                return result;
            }
            Ok(None) => {}
            Err(error) => {
                self.active.lock().await.remove(&session_id);
                return Err(error.into());
            }
        }

        let mut context = workflow.new_context(key);
        let cancellation = match self.harness.start_run(&mut context).await {
            Ok(handle) => handle,
            Err(error) => {
                self.active.lock().await.remove(&session_id);
                return Err(ServiceError::Harness(error));
            }
        };
        {
            let mut active = self.active.lock().await;
            let slot = active.get_mut(&session_id).ok_or(ServiceError::Conflict)?;
            slot.disposition = RunDispositionV1::Running;
            slot.cancellation = Some(cancellation);
        }
        let service = self.clone();
        let harness = self.harness.clone();
        tokio::spawn(async move {
            let _ = workflow.run(harness, context, input).await;
            service.active.lock().await.remove(&session_id);
        });
        Ok(RunView {
            session_id,
            run_id: key.run_id(),
            disposition: RunDispositionV1::Running,
            last_sequence: Some(0),
            outcome: None,
        })
    }

    pub async fn get_run_status(&self, key: RunKey) -> Result<RunView, ServiceError> {
        if let Some(active) = self.active.lock().await.get(&key.session_id())
            && active.key == key
        {
            return Ok(active_view(active));
        }
        disposition_view(self.harness.recover_run(key).await?)
    }

    pub async fn cancel_active_run(&self, key: RunKey) -> Result<(), ServiceError> {
        let handle = {
            let active = self.active.lock().await;
            let slot = active
                .get(&key.session_id())
                .ok_or(ServiceError::Conflict)?;
            if slot.key != key {
                return Err(ServiceError::Conflict);
            }
            slot.cancellation.clone().ok_or(ServiceError::Conflict)?
        };
        handle.request_cancel();
        Ok(())
    }

    pub async fn read_events(
        &self,
        key: RunKey,
        after: Option<EventSequence>,
        limit: u16,
    ) -> Result<Vec<ServiceEventV1>, ServiceError> {
        validate_read_limit(limit)?;
        Ok(self
            .read
            .read_events(key, after, limit)
            .await?
            .events()
            .iter()
            .map(ServiceEventV1::from)
            .collect())
    }

    pub async fn run_page(
        &self,
        after: Option<RunPageCursor>,
        limit: u16,
    ) -> Result<DurableRunPage, ServiceError> {
        validate_read_limit(limit)?;
        self.read.list_runs(after, limit).await.map_err(Into::into)
    }

    pub async fn waiting_page(
        &self,
        after: Option<WaitingPageCursor>,
        limit: u16,
    ) -> Result<(Vec<WaitingApprovalView>, Option<WaitingPageCursor>), ServiceError> {
        validate_read_limit(limit)?;
        let page: DurableWaitingPage = self.read.list_waiting(after, limit).await?;
        let items = page
            .items()
            .iter()
            .map(|item| WaitingApprovalView {
                session_id: item.key().session_id(),
                run_id: item.key().run_id(),
                wait_id: item.wait_id(),
                row_version: item.row_version(),
                state: waiting_state(item.status()),
            })
            .collect();
        Ok((items, page.next()))
    }

    pub async fn approval_preview(
        &self,
        key: RunKey,
        wait_id: DurableApprovalWaitId,
    ) -> Result<ApprovalPreviewV1, ServiceError> {
        let view: DurableApprovalView = self.harness.durable_approval_view(key, wait_id).await?;
        Ok(ApprovalPreviewV1 {
            wait_id,
            row_version: view.row_version(),
            summary: view.preview().summary().to_owned(),
            target: view.preview().target_label().to_owned(),
        })
    }

    pub async fn submit_decision(
        &self,
        key: RunKey,
        wait_id: DurableApprovalWaitId,
        expected_row_version: u64,
        decision: ApprovalDecisionV1,
    ) -> Result<(), ServiceError> {
        let outcome = match decision {
            ApprovalDecisionV1::Approve => DurableApprovalOutcome::Approve,
            ApprovalDecisionV1::Deny => DurableApprovalOutcome::Deny,
        };
        self.harness
            .record_durable_approval_outcome(key, wait_id, expected_row_version, outcome)
            .await
            .map_err(Into::into)
    }

    pub async fn abort_waiting(
        &self,
        key: RunKey,
        wait_id: DurableApprovalWaitId,
        expected_row_version: u64,
    ) -> Result<(), ServiceError> {
        self.harness
            .abort_durable_waiting(key, wait_id, expected_row_version)
            .await
            .map_err(Into::into)
    }

    pub async fn resume_run(
        self: &Arc<Self>,
        key: RunKey,
        wait_id: Option<DurableApprovalWaitId>,
    ) -> Result<(), ServiceError> {
        let disposition = self.harness.recover_run(key).await?;
        if matches!(&disposition, RecoveryDisposition::Waiting(_)) && wait_id.is_none()
            || matches!(&disposition, RecoveryDisposition::Resumable(_)) && wait_id.is_some()
        {
            return Err(ServiceError::InvalidRequest);
        }
        let contract = match &disposition {
            RecoveryDisposition::Waiting(run) => run.recovery_contract(),
            RecoveryDisposition::Resumable(run) => run.recovery_contract(),
            _ => return Err(ServiceError::Conflict),
        };
        let workflow_id = self
            .recovery_workflows
            .iter()
            .find_map(|(candidate, id)| (*candidate == contract).then_some(id))
            .ok_or(ServiceError::WorkflowUnavailable)?;
        let workflow = self
            .workflows
            .get(workflow_id)
            .cloned()
            .ok_or(ServiceError::WorkflowUnavailable)?;
        let cancellation = match &disposition {
            RecoveryDisposition::Waiting(run) => run.cancellation_handle()?,
            RecoveryDisposition::Resumable(run) => run.cancellation_handle()?,
            _ => return Err(ServiceError::Conflict),
        };
        {
            let mut active = self.active.lock().await;
            if active.contains_key(&key.session_id()) || active.len() >= MAX_ACTIVE_RUNS {
                return Err(ServiceError::Conflict);
            }
            active.insert(
                key.session_id(),
                ActiveRun {
                    key,
                    disposition: RunDispositionV1::Running,
                    cancellation: Some(cancellation),
                },
            );
        }
        let service = self.clone();
        let harness = self.harness.clone();
        tokio::spawn(async move {
            match disposition {
                RecoveryDisposition::Waiting(recovered) => {
                    if let Some(wait_id) = wait_id {
                        let _ = workflow.resume_waiting(harness, *recovered, wait_id).await;
                    }
                }
                RecoveryDisposition::Resumable(recovered) => {
                    let _ = workflow.resume_recovered(harness, *recovered).await;
                }
                _ => {}
            }
            service.active.lock().await.remove(&key.session_id());
        });
        Ok(())
    }

    pub async fn discover_runs(&self) -> Result<Vec<RunView>, ServiceError> {
        let mut cursor = None;
        let mut result = Vec::new();
        loop {
            let page = self.read.list_runs(cursor, 256).await?;
            for item in page.items() {
                let view = disposition_view(self.harness.recover_run(item.key()).await?)?;
                self.sessions.lock().await.insert(view.session_id);
                result.push(view);
            }
            cursor = page.next();
            if cursor.is_none() {
                break;
            }
            if result.len() > MAX_SESSIONS {
                return Err(ServiceError::Capacity);
            }
        }
        Ok(result)
    }
}

fn derive_run_id(session_id: SessionId, request_id: Uuid) -> RunId {
    let mut digest = Sha256::new();
    digest.update(b"enterprise-local-agent/service-start/v1");
    digest.update(session_id.as_uuid().as_bytes());
    digest.update(request_id.as_bytes());
    let digest = digest.finalize();
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x50;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    RunId::from_uuid(Uuid::from_bytes(bytes))
}

fn validate_read_limit(limit: u16) -> Result<(), ServiceError> {
    if limit == 0 || limit > MAX_READ_PAGE_ITEMS {
        return Err(ServiceError::InvalidRequest);
    }
    Ok(())
}

fn active_view(active: &ActiveRun) -> RunView {
    RunView {
        session_id: active.key.session_id(),
        run_id: active.key.run_id(),
        disposition: active.disposition,
        last_sequence: None,
        outcome: None,
    }
}

fn disposition_view(disposition: RecoveryDisposition) -> Result<RunView, ServiceError> {
    let (state, kind, outcome) = match disposition {
        RecoveryDisposition::Completed { state } => (
            state,
            RunDispositionV1::Completed,
            Some(RunOutcomeV1::Completed),
        ),
        RecoveryDisposition::TerminalFailure { state, outcome } => (
            state,
            RunDispositionV1::Failed,
            Some(service_outcome(&outcome)),
        ),
        RecoveryDisposition::Resumable(run) => {
            (run.state().clone(), RunDispositionV1::Resumable, None)
        }
        RecoveryDisposition::Waiting(run) => (run.state().clone(), RunDispositionV1::Waiting, None),
        RecoveryDisposition::ManualReconciliationRequired { state, .. } => {
            (state, RunDispositionV1::ManualReconciliationRequired, None)
        }
    };
    Ok(RunView {
        session_id: state.key().session_id(),
        run_id: state.key().run_id(),
        disposition: kind,
        last_sequence: state.last_sequence().map(EventSequence::get),
        outcome,
    })
}

const fn service_outcome(outcome: &RunOutcome) -> RunOutcomeV1 {
    match outcome {
        RunOutcome::Completed => RunOutcomeV1::Completed,
        RunOutcome::Cancelled => RunOutcomeV1::Cancelled,
        RunOutcome::BudgetExceeded { .. } => RunOutcomeV1::BudgetExceeded,
        RunOutcome::Failed { .. } => RunOutcomeV1::Failed,
    }
}

const fn waiting_state(status: DurableApprovalStatus) -> WaitingStateV1 {
    match status {
        DurableApprovalStatus::Waiting => WaitingStateV1::Waiting,
        DurableApprovalStatus::DecisionRecordedApprove | DurableApprovalStatus::ApprovedReady => {
            WaitingStateV1::Approved
        }
        DurableApprovalStatus::DecisionRecordedDeny => WaitingStateV1::Denied,
        DurableApprovalStatus::Executing | DurableApprovalStatus::Consumed => {
            WaitingStateV1::Executing
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ServiceEventCategoryV1 {
    Run,
    Model,
    Action,
    Tool,
    Approval,
    Containment,
    Knowledge,
    Loop,
    Graph,
    Audit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ServiceEventPhaseV1 {
    Started,
    Completed,
    Failed,
    Proposed,
    Validated,
    Rejected,
    Bound,
    Denied,
    Granted,
    Prepared,
    DecisionRecorded,
    Degraded,
    Progress,
    Suspended,
    Resumed,
    Finished,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ServiceEventV1 {
    pub version: u16,
    pub sequence: u64,
    pub run_id: RunId,
    pub category: ServiceEventCategoryV1,
    pub phase: ServiceEventPhaseV1,
    pub correlation_id: Option<String>,
}

impl From<&AgentEvent> for ServiceEventV1 {
    fn from(event: &AgentEvent) -> Self {
        use ServiceEventCategoryV1 as C;
        use ServiceEventPhaseV1 as P;
        let (category, phase, id) = match event.kind() {
            AgentEventKind::RunStarted { .. } => (C::Run, P::Started, None),
            AgentEventKind::RunFinished { .. } => (C::Run, P::Finished, None),
            AgentEventKind::AuditDegraded => (C::Audit, P::Degraded, None),
            AgentEventKind::ModelInvocationStarted { model_call_id, .. } => {
                (C::Model, P::Started, Some(model_call_id.to_string()))
            }
            AgentEventKind::ModelInvocationCompleted { model_call_id, .. } => {
                (C::Model, P::Completed, Some(model_call_id.to_string()))
            }
            AgentEventKind::ModelInvocationFailed { model_call_id } => {
                (C::Model, P::Failed, Some(model_call_id.to_string()))
            }
            AgentEventKind::ActionProposed {
                action_proposal_id, ..
            } => (C::Action, P::Proposed, Some(action_proposal_id.to_string())),
            AgentEventKind::ActionValidated { action_proposal_id } => (
                C::Action,
                P::Validated,
                Some(action_proposal_id.to_string()),
            ),
            AgentEventKind::ActionRejected {
                action_proposal_id, ..
            } => (C::Action, P::Rejected, Some(action_proposal_id.to_string())),
            AgentEventKind::ActionExecutionBound { tool_call_id, .. } => {
                (C::Action, P::Bound, Some(tool_call_id.to_string()))
            }
            AgentEventKind::ToolInvocationStarted { tool_call_id, .. } => {
                (C::Tool, P::Started, Some(tool_call_id.to_string()))
            }
            AgentEventKind::ToolInvocationCompleted { tool_call_id } => {
                (C::Tool, P::Completed, Some(tool_call_id.to_string()))
            }
            AgentEventKind::ToolInvocationDomainFailed { tool_call_id, .. }
            | AgentEventKind::ToolInvocationAdapterFailed { tool_call_id } => {
                (C::Tool, P::Failed, Some(tool_call_id.to_string()))
            }
            AgentEventKind::ToolPolicyDenied { tool_call_id, .. } => {
                (C::Tool, P::Denied, Some(tool_call_id.to_string()))
            }
            AgentEventKind::ApprovalRequested {
                approval_request_id,
                ..
            } => (
                C::Approval,
                P::Started,
                Some(approval_request_id.to_string()),
            ),
            AgentEventKind::ApprovalGranted {
                approval_request_id,
            }
            | AgentEventKind::DurableApprovalGranted {
                approval_request_id,
                ..
            } => (
                C::Approval,
                P::Granted,
                Some(approval_request_id.to_string()),
            ),
            AgentEventKind::ApprovalDenied {
                approval_request_id,
            }
            | AgentEventKind::DurableApprovalDenied {
                approval_request_id,
                ..
            } => (
                C::Approval,
                P::Denied,
                Some(approval_request_id.to_string()),
            ),
            AgentEventKind::ApprovalFailed {
                approval_request_id,
                ..
            } => (
                C::Approval,
                P::Failed,
                Some(approval_request_id.to_string()),
            ),
            AgentEventKind::DurableApprovalPrepared { wait_id, .. } => {
                (C::Approval, P::Prepared, Some(wait_id.to_string()))
            }
            AgentEventKind::DurableApprovalDecisionRecorded { wait_id, .. } => {
                (C::Approval, P::DecisionRecorded, Some(wait_id.to_string()))
            }
            AgentEventKind::ContainmentFailed { tool_call_id, .. } => {
                (C::Containment, P::Failed, Some(tool_call_id.to_string()))
            }
            AgentEventKind::KnowledgeRetrievalStarted { retrieval_id, .. }
            | AgentEventKind::KnowledgeRetrievalRestarted { retrieval_id, .. } => {
                (C::Knowledge, P::Started, Some(retrieval_id.to_string()))
            }
            AgentEventKind::KnowledgeRetrievalCompleted { retrieval_id, .. } => {
                (C::Knowledge, P::Completed, Some(retrieval_id.to_string()))
            }
            AgentEventKind::KnowledgeRetrievalFailed { retrieval_id, .. } => {
                (C::Knowledge, P::Failed, Some(retrieval_id.to_string()))
            }
            AgentEventKind::ModelGroundingBound { model_call_id, .. } => {
                (C::Knowledge, P::Bound, Some(model_call_id.to_string()))
            }
            AgentEventKind::Loop { .. } => (C::Loop, P::Progress, None),
            AgentEventKind::Graph { event } => {
                use agent_core::GraphProgressEvent;
                match event {
                    GraphProgressEvent::GraphStarted { .. }
                    | GraphProgressEvent::GraphNodeEntered { .. }
                    | GraphProgressEvent::GraphNodeRestarted { .. } => (C::Graph, P::Started, None),
                    GraphProgressEvent::GraphNodeCompleted { .. }
                    | GraphProgressEvent::GraphCompleted { .. } => (C::Graph, P::Completed, None),
                    GraphProgressEvent::GraphSuspended { wait_id, .. } => {
                        (C::Graph, P::Suspended, Some(wait_id.to_string()))
                    }
                    GraphProgressEvent::GraphResumed { wait_id, .. } => {
                        (C::Graph, P::Resumed, Some(wait_id.to_string()))
                    }
                    GraphProgressEvent::GraphFailed { .. } => (C::Graph, P::Failed, None),
                }
            }
        };
        Self {
            version: SERVICE_EVENT_VERSION,
            sequence: event.sequence().get(),
            run_id: event.run_id(),
            category,
            phase,
            correlation_id: id,
        }
    }
}

#[derive(Debug, Error)]
pub enum ServiceError {
    #[error("invalid request")]
    InvalidRequest,
    #[error("resource not found")]
    NotFound,
    #[error("request conflicts with current state")]
    Conflict,
    #[error("service capacity exhausted")]
    Capacity,
    #[error("workflow unavailable")]
    WorkflowUnavailable,
    #[error("service configuration is invalid")]
    Configuration,
    #[error("governed runtime rejected the operation")]
    Harness(#[from] agent_harness::HarnessError),
    #[error("durable read failed")]
    Persistence(#[from] PersistencePortError),
    #[error("run context is unavailable")]
    Context(#[from] agent_harness::RunContextError),
}

#[cfg(test)]
mod tests {
    use std::{
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        },
        time::Duration,
    };

    use agent_core::{ModelResponse, RunBudget};
    use agent_harness::{
        AuditFailurePolicy, HarnessConfig, M0ReadOnlyPolicy, ToolRegistry,
        testing::{FakeModelPort, InMemoryAuditSink},
    };
    use agent_persistence_sqlite::SqliteRunPersistence;

    use super::*;

    struct TestWorkflow {
        id: WorkflowId,
        invocations: Arc<AtomicUsize>,
        delay: Duration,
    }

    impl ConfiguredWorkflow for TestWorkflow {
        fn id(&self) -> &WorkflowId {
            &self.id
        }
        fn recovery_contract(&self) -> RecoveryContract {
            RecoveryContract::NonRestartable
        }
        fn new_context(&self, key: RunKey) -> RunContext {
            RunContext::new(
                key.run_id(),
                key.session_id(),
                RunBudget::new(1, 1, 1, Duration::from_secs(10)).expect("budget"),
            )
        }
        fn run(
            self: Arc<Self>,
            harness: Arc<ExecutionHarness>,
            mut context: RunContext,
            _input: RunInput,
        ) -> ServiceFuture<'static, Result<(), WorkflowError>> {
            Box::pin(async move {
                self.invocations.fetch_add(1, Ordering::SeqCst);
                tokio::time::sleep(self.delay).await;
                match harness.checkpoint(&mut context).await {
                    Ok(()) => harness
                        .complete_run(&mut context)
                        .await
                        .map_err(|_| WorkflowError::Failed),
                    Err(_) => Ok(()),
                }
            })
        }
        fn resume_waiting(
            self: Arc<Self>,
            _harness: Arc<ExecutionHarness>,
            _recovered: RecoveredWaitingRun,
            _wait_id: DurableApprovalWaitId,
        ) -> ServiceFuture<'static, Result<(), WorkflowError>> {
            Box::pin(async { Err(WorkflowError::NotRestartable) })
        }
    }

    async fn fixture(delay: Duration) -> (tempfile::TempDir, Arc<AgentService>, Arc<AtomicUsize>) {
        let directory = tempfile::tempdir().expect("directory");
        let store = Arc::new(
            SqliteRunPersistence::open(directory.path().join("runs.sqlite3"))
                .await
                .expect("store"),
        );
        let harness = Arc::new(
            ExecutionHarness::new(
                Arc::new(FakeModelPort::scripted(vec![Ok(ModelResponse::new(
                    vec![],
                    None,
                ))])),
                ToolRegistry::new(),
                Arc::new(M0ReadOnlyPolicy),
                Arc::new(InMemoryAuditSink::new()),
                HarnessConfig::new(AuditFailurePolicy::FailClosed, Duration::from_secs(1))
                    .expect("config"),
            )
            .with_persistence_port(store.clone()),
        );
        let invocations = Arc::new(AtomicUsize::new(0));
        let workflow: Arc<dyn ConfiguredWorkflow> = Arc::new(TestWorkflow {
            id: WorkflowId::new("test").expect("id"),
            invocations: invocations.clone(),
            delay,
        });
        let service = Arc::new(AgentService::new(harness, store, vec![workflow]).expect("service"));
        (directory, service, invocations)
    }

    #[tokio::test]
    async fn duplicate_start_is_idempotent_and_distinct_concurrent_start_conflicts() {
        let (_directory, service, invocations) = fixture(Duration::from_millis(100)).await;
        let session = service.create_session().await.expect("session");
        let request = Uuid::new_v4();
        let workflow = WorkflowId::new("test").expect("workflow");
        let first = service.start_run(
            session,
            request,
            &workflow,
            RunInput::new(b"one".to_vec()).expect("input"),
        );
        let second = service.start_run(
            session,
            request,
            &workflow,
            RunInput::new(b"one".to_vec()).expect("input"),
        );
        let (first, second) = tokio::join!(first, second);
        let first = first.expect("first");
        let second = second.expect("idempotent retry");
        assert_eq!(first.run_id, second.run_id);
        assert!(matches!(
            service
                .start_run(
                    session,
                    Uuid::new_v4(),
                    &workflow,
                    RunInput::new(b"two".to_vec()).expect("input")
                )
                .await,
            Err(ServiceError::Conflict)
        ));
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert_eq!(invocations.load(Ordering::SeqCst), 1);
        let status = service
            .get_run_status(RunKey::new(first.run_id, session))
            .await
            .expect("status");
        assert_eq!(status.disposition, RunDispositionV1::Completed);
        let retry = service
            .start_run(
                session,
                request,
                &workflow,
                RunInput::new(b"changed".to_vec()).expect("input"),
            )
            .await
            .expect("durable retry");
        assert_eq!(retry.run_id, first.run_id);
        assert_eq!(invocations.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn cancellation_and_event_projection_remain_metadata_only() {
        let (_directory, service, _) = fixture(Duration::from_millis(100)).await;
        let session = service.create_session().await.expect("session");
        let workflow = WorkflowId::new("test").expect("workflow");
        let run = service
            .start_run(
                session,
                Uuid::new_v4(),
                &workflow,
                RunInput::new(b"secret-prompt".to_vec()).expect("input"),
            )
            .await
            .expect("start");
        let key = RunKey::new(run.run_id, session);
        service.cancel_active_run(key).await.expect("cancel");
        tokio::time::sleep(Duration::from_millis(150)).await;
        let events = service.read_events(key, None, 64).await.expect("events");
        let encoded = serde_json::to_string(&events).expect("encode");
        assert!(!encoded.contains("secret-prompt"));
        assert_eq!(events.first().map(|event| event.sequence), Some(0));
        assert!(
            events
                .windows(2)
                .all(|events| events[1].sequence == events[0].sequence + 1)
        );
        assert_eq!(
            service
                .get_run_status(key)
                .await
                .expect("status")
                .disposition,
            RunDispositionV1::Failed
        );
    }

    #[tokio::test]
    async fn restart_discovers_run_and_restores_its_session() {
        let (directory, first, _) = fixture(Duration::from_millis(1)).await;
        let session = first.create_session().await.expect("session");
        let workflow_id = WorkflowId::new("test").expect("workflow");
        let run = first
            .start_run(
                session,
                Uuid::new_v4(),
                &workflow_id,
                RunInput::new(b"first".to_vec()).expect("input"),
            )
            .await
            .expect("start");
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert_eq!(
            first
                .get_run_status(RunKey::new(run.run_id, session))
                .await
                .expect("status")
                .disposition,
            RunDispositionV1::Completed
        );

        let store = Arc::new(
            SqliteRunPersistence::open(directory.path().join("runs.sqlite3"))
                .await
                .expect("reopen store"),
        );
        let harness = Arc::new(
            ExecutionHarness::new(
                Arc::new(FakeModelPort::scripted(Vec::new())),
                ToolRegistry::new(),
                Arc::new(M0ReadOnlyPolicy),
                Arc::new(InMemoryAuditSink::new()),
                HarnessConfig::new(AuditFailurePolicy::FailClosed, Duration::from_secs(1))
                    .expect("config"),
            )
            .with_persistence_port(store.clone()),
        );
        let workflow: Arc<dyn ConfiguredWorkflow> = Arc::new(TestWorkflow {
            id: workflow_id.clone(),
            invocations: Arc::new(AtomicUsize::new(0)),
            delay: Duration::from_millis(1),
        });
        let second =
            Arc::new(AgentService::new(harness, store, vec![workflow]).expect("restarted service"));
        let discovered = second.discover_runs().await.expect("discover");
        assert_eq!(discovered.len(), 1);
        assert_eq!(discovered[0].disposition, RunDispositionV1::Completed);
        second
            .start_run(
                session,
                Uuid::new_v4(),
                &workflow_id,
                RunInput::new(b"second".to_vec()).expect("input"),
            )
            .await
            .expect("recovered session accepts new run");
    }
}
