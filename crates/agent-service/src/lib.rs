use std::{
    collections::{HashMap, HashSet},
    fmt,
    future::Future,
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicU8, Ordering},
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use agent_core::{AgentEvent, AgentEventKind, DurableApprovalOutcome, RunBudget, RunOutcome};
pub use agent_core::{DurableApprovalWaitId, EventSequence, RunId, SessionId};
use agent_harness::{
    DurableApprovalStatus, DurableApprovalView, DurableRunPage, DurableRunSummary,
    DurableWaitingPage, ExecutionHarness, MAX_READ_PAGE_ITEMS, PersistencePortError, RecoveredRun,
    RecoveredWaitingRun, RecoveryContract, RecoveryDisposition, RunCancellationHandle, RunContext,
    RunPageCursor, RunReadPort,
};
pub use agent_harness::{RunKey, WaitingPageCursor};
use agent_identity::{
    AuthorizationAction, AuthorizationDecision, AuthorizationResource, DurableRunAuthorization,
    PrincipalKind, PrincipalRole, SecurityAuditEvent, SecurityAuditPhase, SecurityAuditPort,
    ServiceAuthorizationPolicy, SessionOwnershipPort, SessionOwnershipRecord, SessionPageCursor,
    VerifiedPrincipal,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;
use tokio::sync::Mutex;
use tokio_util::task::TaskTracker;
use uuid::Uuid;

pub const MAX_RUN_INPUT_BYTES: usize = 8 * 1024;
pub const MAX_ACTIVE_RUNS: usize = 32;
pub const MAX_SESSIONS: usize = 256;
pub const MAX_COMMAND_QUEUE: usize = 32;
pub const MAX_APPLICATION_RESULTS: usize = 256;
pub const MAX_FINAL_ANSWER_BYTES: usize = 8 * 1024;
pub const MAX_RESULT_CITATIONS: usize = 8;
pub const MAX_CITATION_FIELD_BYTES: usize = 512;
pub const SERVICE_EVENT_VERSION: u16 = 2;

pub trait ServiceObserver: Send + Sync {
    fn run_started(&self) {}
    fn run_settled(&self, _disposition: Option<RunDispositionV1>, _duration: Duration) {}
}

struct NoopServiceObserver;

impl ServiceObserver for NoopServiceObserver {}

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
    pub workflow_id: Option<WorkflowId>,
    pub result: Option<ApplicationResultV1>,
    pub duration_millis: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RunHistoryItemV1 {
    pub run_id: RunId,
    pub disposition: RunDispositionV1,
    pub last_sequence: Option<u64>,
    pub outcome: Option<RunOutcomeV1>,
    pub workflow_id: Option<WorkflowId>,
    pub started_at_unix_millis: Option<u64>,
    pub result_available: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RunHistoryPageV1 {
    pub items: Vec<RunHistoryItemV1>,
    pub next_run_id: Option<RunId>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ConversationSummaryV1 {
    pub session_id: SessionId,
    pub title: String,
    pub last_activity_unix_millis: Option<u64>,
    pub latest_run: Option<RunHistoryItemV1>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ConversationPageV1 {
    pub items: Vec<ConversationSummaryV1>,
    pub next_session_id: Option<SessionId>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct OperationalRunViewV1 {
    pub session_id: SessionId,
    pub run_id: RunId,
    pub disposition: RunDispositionV1,
    pub last_sequence: Option<u64>,
    pub workflow_id: Option<WorkflowId>,
    pub duration_millis: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct OperationalRunPageV1 {
    pub items: Vec<OperationalRunViewV1>,
    pub next_run_id: Option<RunId>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PrincipalViewV1 {
    pub principal_id: agent_core::PrincipalId,
    pub kind: PrincipalKind,
    pub roles: Vec<PrincipalRole>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowBudgetV1 {
    pub workflow_id: WorkflowId,
    pub max_model_calls: u32,
    pub max_tool_calls: u32,
    pub max_iterations: u32,
    pub max_approval_requests: u32,
    pub max_graph_steps: u32,
    pub max_elapsed_millis: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeStatusV1 {
    pub principal: PrincipalViewV1,
    pub max_active_runs: u16,
    pub max_run_input_bytes: u32,
    pub max_read_page_items: u16,
    pub workflow_budgets: Vec<WorkflowBudgetV1>,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApplicationCitationV1 {
    pub evidence_id: String,
    pub backend: agent_core::KnowledgeBackendId,
    pub source_id: String,
    pub reference_id: String,
    pub provenance: Option<String>,
}

impl fmt::Debug for ApplicationCitationV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ApplicationCitationV1")
            .field("evidence_id", &self.evidence_id)
            .field("backend", &self.backend)
            .field("content", &"[REDACTED]")
            .finish_non_exhaustive()
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind", deny_unknown_fields)]
pub enum ApplicationResultV1 {
    FinalAnswer {
        answer: String,
        citations: Vec<ApplicationCitationV1>,
    },
    LocalWriteCompleted {
        tool_call_id: agent_core::ToolCallId,
    },
    ApprovalDenied,
}

impl fmt::Debug for ApplicationResultV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::FinalAnswer { answer, citations } => formatter
                .debug_struct("ApplicationResultV1::FinalAnswer")
                .field("answer_bytes", &answer.len())
                .field("citation_count", &citations.len())
                .field("content", &"[REDACTED]")
                .finish(),
            Self::LocalWriteCompleted { tool_call_id } => formatter
                .debug_struct("ApplicationResultV1::LocalWriteCompleted")
                .field("tool_call_id", tool_call_id)
                .finish(),
            Self::ApprovalDenied => formatter.write_str("ApplicationResultV1::ApprovalDenied"),
        }
    }
}

impl ApplicationResultV1 {
    pub fn final_answer(
        answer: String,
        citations: Vec<ApplicationCitationV1>,
    ) -> Result<Self, ServiceError> {
        if answer.is_empty()
            || answer.len() > MAX_FINAL_ANSWER_BYTES
            || citations.len() > MAX_RESULT_CITATIONS
            || citations.iter().any(|citation| {
                [
                    citation.evidence_id.as_str(),
                    citation.source_id.as_str(),
                    citation.reference_id.as_str(),
                ]
                .into_iter()
                .chain(citation.provenance.as_deref())
                .any(|value| {
                    value.is_empty()
                        || value.len() > MAX_CITATION_FIELD_BYTES
                        || value.chars().any(char::is_control)
                })
            })
        {
            return Err(ServiceError::InvalidRequest);
        }
        Ok(Self::FinalAnswer { answer, citations })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WorkflowCompletion {
    NoApplicationResult,
    ApplicationResult(ApplicationResultV1),
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
    fn budget(&self) -> RunBudget;
    fn recovery_contract(&self) -> RecoveryContract;
    fn validate_input(&self, _input: &RunInput) -> Result<(), WorkflowError> {
        Ok(())
    }
    fn new_context(&self, key: RunKey) -> RunContext;
    fn run(
        self: Arc<Self>,
        harness: Arc<ExecutionHarness>,
        context: RunContext,
        input: RunInput,
    ) -> ServiceFuture<'static, Result<WorkflowCompletion, WorkflowError>>;
    fn resume_waiting(
        self: Arc<Self>,
        harness: Arc<ExecutionHarness>,
        recovered: RecoveredWaitingRun,
        wait_id: DurableApprovalWaitId,
    ) -> ServiceFuture<'static, Result<WorkflowCompletion, WorkflowError>>;
    fn resume_recovered(
        self: Arc<Self>,
        _harness: Arc<ExecutionHarness>,
        _recovered: RecoveredRun,
    ) -> ServiceFuture<'static, Result<WorkflowCompletion, WorkflowError>> {
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
    workflow_id: WorkflowId,
    started_at: Instant,
}

#[derive(Clone)]
struct StoredApplicationResult {
    result: ApplicationResultV1,
    duration_millis: u64,
}

pub struct AgentService {
    harness: Arc<ExecutionHarness>,
    read: Arc<dyn RunReadPort>,
    ownership: Arc<dyn SessionOwnershipPort>,
    authorization: Arc<dyn ServiceAuthorizationPolicy>,
    security_audit: Arc<dyn SecurityAuditPort>,
    workflows: HashMap<WorkflowId, Arc<dyn ConfiguredWorkflow>>,
    recovery_workflows: Vec<(RecoveryContract, WorkflowId)>,
    sessions: Mutex<HashSet<SessionId>>,
    active: Mutex<HashMap<SessionId, ActiveRun>>,
    results: Mutex<HashMap<RunKey, StoredApplicationResult>>,
    lifecycle: AtomicU8,
    tasks: TaskTracker,
    observer: Arc<dyn ServiceObserver>,
}

const LIFECYCLE_SERVING: u8 = 1;
const LIFECYCLE_DRAINING: u8 = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ServiceLifecycleV1 {
    Serving,
    Draining,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ShutdownReportV1 {
    pub cancelled_active_runs: u16,
    pub remaining_active_runs: u16,
}

impl AgentService {
    pub fn new(
        harness: Arc<ExecutionHarness>,
        read: Arc<dyn RunReadPort>,
        ownership: Arc<dyn SessionOwnershipPort>,
        authorization: Arc<dyn ServiceAuthorizationPolicy>,
        security_audit: Arc<dyn SecurityAuditPort>,
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
            ownership,
            authorization,
            security_audit,
            workflows: by_id,
            recovery_workflows: by_recovery,
            sessions: Mutex::new(HashSet::new()),
            active: Mutex::new(HashMap::new()),
            results: Mutex::new(HashMap::new()),
            lifecycle: AtomicU8::new(LIFECYCLE_SERVING),
            tasks: TaskTracker::new(),
            observer: Arc::new(NoopServiceObserver),
        })
    }

    #[must_use]
    pub fn with_observer(mut self, observer: Arc<dyn ServiceObserver>) -> Self {
        self.observer = observer;
        self
    }

    #[must_use]
    pub fn lifecycle(&self) -> ServiceLifecycleV1 {
        if self.lifecycle.load(Ordering::Acquire) == LIFECYCLE_DRAINING {
            ServiceLifecycleV1::Draining
        } else {
            ServiceLifecycleV1::Serving
        }
    }

    fn ensure_serving(&self) -> Result<(), ServiceError> {
        (self.lifecycle() == ServiceLifecycleV1::Serving)
            .then_some(())
            .ok_or(ServiceError::Draining)
    }

    fn authorize(
        &self,
        principal: &VerifiedPrincipal,
        action: AuthorizationAction,
        resource: AuthorizationResource<'_>,
    ) -> Result<(), ServiceError> {
        (self.authorization.authorize(principal, action, resource)
            == AuthorizationDecision::Granted)
            .then_some(())
            .ok_or(ServiceError::Unauthorized)
    }

    async fn audit_security(
        &self,
        principal: &VerifiedPrincipal,
        action: AuthorizationAction,
        phase: SecurityAuditPhase,
        key: Option<RunKey>,
        wait_id: Option<DurableApprovalWaitId>,
    ) -> Result<(), ServiceError> {
        self.security_audit
            .record(&SecurityAuditEvent {
                phase,
                actor: principal.id().clone(),
                action,
                session_id: key.map(RunKey::session_id),
                run_id: key.map(RunKey::run_id),
                wait_id,
            })
            .await
            .map_err(|_| ServiceError::SecurityAudit)
    }

    async fn authorize_mutation(
        &self,
        principal: &VerifiedPrincipal,
        action: AuthorizationAction,
        resource: AuthorizationResource<'_>,
        key: Option<RunKey>,
        wait_id: Option<DurableApprovalWaitId>,
    ) -> Result<(), ServiceError> {
        self.authorize(principal, action, resource)?;
        self.audit_security(
            principal,
            action,
            SecurityAuditPhase::AuthorizationGranted,
            key,
            wait_id,
        )
        .await?;
        self.audit_security(
            principal,
            action,
            SecurityAuditPhase::MutationRequested,
            key,
            wait_id,
        )
        .await
    }

    async fn audit_mutation_result<T>(
        &self,
        principal: &VerifiedPrincipal,
        action: AuthorizationAction,
        key: Option<RunKey>,
        wait_id: Option<DurableApprovalWaitId>,
        result: Result<T, ServiceError>,
    ) -> Result<T, ServiceError> {
        let phase = if result.is_ok() {
            SecurityAuditPhase::MutationCommitted
        } else {
            SecurityAuditPhase::MutationFailed
        };
        self.audit_security(principal, action, phase, key, wait_id)
            .await?;
        result
    }

    async fn run_authorization(
        &self,
        key: RunKey,
    ) -> Result<DurableRunAuthorization, ServiceError> {
        self.read
            .find_run(key)
            .await?
            .ok_or(ServiceError::NotFound)?
            .authorization()
            .cloned()
            .ok_or(ServiceError::LegacyUnowned)
    }

    pub async fn drain(&self, deadline: Duration) -> Result<ShutdownReportV1, ServiceError> {
        self.lifecycle.store(LIFECYCLE_DRAINING, Ordering::Release);
        self.tasks.close();
        let handles = {
            let active = self.active.lock().await;
            active
                .values()
                .filter_map(|run| run.cancellation.clone())
                .collect::<Vec<_>>()
        };
        let cancelled_active_runs = u16::try_from(handles.len()).unwrap_or(u16::MAX);
        for handle in handles {
            handle.request_cancel();
        }
        tokio::time::timeout(deadline, self.tasks.wait())
            .await
            .map_err(|_| ServiceError::Conflict)?;
        let remaining_active_runs =
            u16::try_from(self.active.lock().await.len()).unwrap_or(u16::MAX);
        if remaining_active_runs != 0 {
            return Err(ServiceError::Conflict);
        }
        Ok(ShutdownReportV1 {
            cancelled_active_runs,
            remaining_active_runs,
        })
    }

    pub async fn create_session(
        &self,
        principal: &VerifiedPrincipal,
    ) -> Result<SessionId, ServiceError> {
        self.ensure_serving()?;
        self.authorize_mutation(
            principal,
            AuthorizationAction::CreateSession,
            AuthorizationResource::Global,
            None,
            None,
        )
        .await?;
        if self.sessions.lock().await.len() >= MAX_SESSIONS {
            return Err(ServiceError::Capacity);
        }
        let id = SessionId::new();
        let result = self
            .ownership
            .create_session(&SessionOwnershipRecord::new(id, principal.id().clone()))
            .await
            .map_err(ServiceError::from);
        self.audit_mutation_result(
            principal,
            AuthorizationAction::CreateSession,
            None,
            None,
            result,
        )
        .await?;
        self.sessions.lock().await.insert(id);
        Ok(id)
    }

    pub async fn start_run(
        self: &Arc<Self>,
        principal: &VerifiedPrincipal,
        session_id: SessionId,
        start_request_id: Uuid,
        workflow_id: &WorkflowId,
        input: RunInput,
    ) -> Result<RunView, ServiceError> {
        self.ensure_serving()?;
        workflow_id.validate()?;
        let session = self
            .ownership
            .load_session(session_id)
            .await?
            .ok_or(ServiceError::NotFound)?;
        if session.owner() != principal.id() {
            return Err(ServiceError::Unauthorized);
        }
        self.authorize_mutation(
            principal,
            AuthorizationAction::StartWorkflow,
            AuthorizationResource::Workflow {
                workflow_id: workflow_id.as_str(),
            },
            None,
            None,
        )
        .await?;
        let workflow = self
            .workflows
            .get(workflow_id)
            .cloned()
            .ok_or(ServiceError::WorkflowUnavailable)?;
        workflow
            .validate_input(&input)
            .map_err(|_| ServiceError::InvalidRequest)?;
        let key = RunKey::new(derive_run_id(session_id, start_request_id), session_id);
        {
            let mut active = self.active.lock().await;
            self.ensure_serving()?;
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
                    workflow_id: workflow_id.clone(),
                    started_at: Instant::now(),
                },
            );
        }

        match self.read.find_run(key).await {
            Ok(Some(summary)) => {
                let authorization = summary.authorization().ok_or(ServiceError::LegacyUnowned)?;
                if authorization.owner() != principal.id()
                    || authorization.workflow_id() != workflow_id.as_str()
                {
                    self.active.lock().await.remove(&session_id);
                    return Err(ServiceError::Unauthorized);
                }
                let result = match self.harness.recover_run(key).await {
                    Ok(disposition) => self.view_for_disposition(disposition).await,
                    Err(error) => Err(ServiceError::from(error)),
                };
                self.active.lock().await.remove(&session_id);
                return self
                    .audit_mutation_result(
                        principal,
                        AuthorizationAction::StartWorkflow,
                        Some(key),
                        None,
                        result,
                    )
                    .await;
            }
            Ok(None) => {}
            Err(error) => {
                self.active.lock().await.remove(&session_id);
                return Err(error.into());
            }
        }

        let mut context = workflow.new_context(key);
        let durable_authorization = DurableRunAuthorization::new(
            principal.id().clone(),
            principal.id().clone(),
            workflow_id.as_str().to_owned(),
            self.authorization.policy_version(),
            self.authorization.fingerprint(),
        )
        .map_err(|_| ServiceError::Configuration)?;
        let cancellation = match self
            .harness
            .start_run_with_authorization(&mut context, durable_authorization)
            .await
        {
            Ok(handle) => handle,
            Err(error) => {
                self.active.lock().await.remove(&session_id);
                let _ = self
                    .audit_security(
                        principal,
                        AuthorizationAction::StartWorkflow,
                        SecurityAuditPhase::MutationFailed,
                        Some(key),
                        None,
                    )
                    .await;
                return Err(ServiceError::Harness(error));
            }
        };
        self.audit_mutation_result(
            principal,
            AuthorizationAction::StartWorkflow,
            Some(key),
            None,
            Ok(()),
        )
        .await?;
        {
            let mut active = self.active.lock().await;
            let slot = active.get_mut(&session_id).ok_or(ServiceError::Conflict)?;
            slot.disposition = RunDispositionV1::Running;
            slot.cancellation = Some(cancellation);
        }
        self.observer.run_started();
        if self.lifecycle() == ServiceLifecycleV1::Draining
            && let Some(handle) = self
                .active
                .lock()
                .await
                .get(&session_id)
                .and_then(|run| run.cancellation.clone())
        {
            handle.request_cancel();
        }
        let service = self.clone();
        let harness = self.harness.clone();
        self.tasks.spawn(async move {
            let started = Instant::now();
            let completion = workflow.run(harness, context, input).await;
            if let Ok(WorkflowCompletion::ApplicationResult(result)) = completion.as_ref() {
                service.store_result(key, result.clone()).await;
            }
            let disposition = service
                .harness
                .recover_run(key)
                .await
                .ok()
                .map(|value| disposition_kind(&value));
            service.observer.run_settled(disposition, started.elapsed());
            service.active.lock().await.remove(&session_id);
        });
        Ok(RunView {
            session_id,
            run_id: key.run_id(),
            disposition: RunDispositionV1::Running,
            last_sequence: Some(0),
            outcome: None,
            workflow_id: Some(workflow_id.clone()),
            result: None,
            duration_millis: None,
        })
    }

    pub async fn get_run_status(
        &self,
        principal: &VerifiedPrincipal,
        key: RunKey,
    ) -> Result<RunView, ServiceError> {
        let authorization = self.run_authorization(key).await?;
        self.authorize(
            principal,
            AuthorizationAction::ReadRun,
            AuthorizationResource::OwnedRun {
                workflow_id: authorization.workflow_id(),
                owner: authorization.owner(),
            },
        )?;
        if let Some(active) = self.active.lock().await.get(&key.session_id())
            && active.key == key
        {
            return Ok(active_view(active));
        }
        self.view_for_disposition(self.harness.recover_run(key).await?)
            .await
    }

    pub fn runtime_status(
        &self,
        principal: &VerifiedPrincipal,
    ) -> Result<RuntimeStatusV1, ServiceError> {
        self.authorize(
            principal,
            AuthorizationAction::ReadRuntimeStatus,
            AuthorizationResource::Global,
        )?;
        let mut workflow_budgets = self
            .workflows
            .values()
            .map(|workflow| {
                let budget = workflow.budget();
                let max_elapsed_millis = u64::try_from(budget.max_elapsed().as_millis())
                    .map_err(|_| ServiceError::Configuration)?;
                Ok(WorkflowBudgetV1 {
                    workflow_id: workflow.id().clone(),
                    max_model_calls: budget.max_model_calls(),
                    max_tool_calls: budget.max_tool_calls(),
                    max_iterations: budget.max_iterations(),
                    max_approval_requests: budget.max_approval_requests(),
                    max_graph_steps: budget.max_graph_steps(),
                    max_elapsed_millis,
                })
            })
            .collect::<Result<Vec<_>, ServiceError>>()?;
        workflow_budgets
            .sort_by(|left, right| left.workflow_id.as_str().cmp(right.workflow_id.as_str()));
        let roles = [
            PrincipalRole::User,
            PrincipalRole::Approver,
            PrincipalRole::Operator,
        ]
        .into_iter()
        .filter(|role| principal.has_role(*role))
        .collect();
        Ok(RuntimeStatusV1 {
            principal: PrincipalViewV1 {
                principal_id: principal.id().clone(),
                kind: principal.kind(),
                roles,
            },
            max_active_runs: MAX_ACTIVE_RUNS as u16,
            max_run_input_bytes: MAX_RUN_INPUT_BYTES as u32,
            max_read_page_items: MAX_READ_PAGE_ITEMS,
            workflow_budgets,
        })
    }

    pub async fn conversation_page(
        &self,
        principal: &VerifiedPrincipal,
        after: Option<SessionPageCursor>,
        limit: u16,
    ) -> Result<ConversationPageV1, ServiceError> {
        validate_read_limit(limit)?;
        self.authorize(
            principal,
            AuthorizationAction::ListSessions,
            AuthorizationResource::Global,
        )?;
        let page = self
            .ownership
            .list_sessions(principal.id(), after, limit)
            .await?;
        let mut items = Vec::with_capacity(page.items().len());
        for session in page.items() {
            if session.owner() != principal.id() {
                return Err(ServiceError::Unauthorized);
            }
            let latest = self
                .read
                .list_session_runs(session.session_id(), None, 1)
                .await?;
            let latest_run = match latest.items().first() {
                Some(summary) => Some(self.history_item(summary).await?),
                None => None,
            };
            items.push(ConversationSummaryV1 {
                session_id: session.session_id(),
                title: conversation_title(session.session_id()),
                last_activity_unix_millis: latest_run
                    .as_ref()
                    .and_then(|run| run.started_at_unix_millis),
                latest_run,
            });
        }
        Ok(ConversationPageV1 {
            items,
            next_session_id: page.next().map(SessionPageCursor::session_id),
        })
    }

    pub async fn session_run_page(
        &self,
        principal: &VerifiedPrincipal,
        session_id: SessionId,
        after: Option<RunPageCursor>,
        limit: u16,
    ) -> Result<RunHistoryPageV1, ServiceError> {
        validate_read_limit(limit)?;
        let session = self
            .ownership
            .load_session(session_id)
            .await?
            .ok_or(ServiceError::NotFound)?;
        self.authorize(
            principal,
            AuthorizationAction::ReadSessionHistory,
            AuthorizationResource::OwnedSession {
                owner: session.owner(),
            },
        )?;
        let page = self
            .read
            .list_session_runs(session_id, after, limit)
            .await?;
        let mut items = Vec::with_capacity(page.items().len());
        for summary in page.items() {
            let authorization = summary.authorization().ok_or(ServiceError::LegacyUnowned)?;
            if authorization.owner() != session.owner() {
                return Err(ServiceError::Unauthorized);
            }
            items.push(self.history_item(summary).await?);
        }
        Ok(RunHistoryPageV1 {
            items,
            next_run_id: page.next().map(RunPageCursor::run_id),
        })
    }

    pub async fn cancel_active_run(
        &self,
        principal: &VerifiedPrincipal,
        key: RunKey,
    ) -> Result<(), ServiceError> {
        let authorization = self.run_authorization(key).await?;
        self.authorize_mutation(
            principal,
            AuthorizationAction::CancelRun,
            AuthorizationResource::OwnedRun {
                workflow_id: authorization.workflow_id(),
                owner: authorization.owner(),
            },
            Some(key),
            None,
        )
        .await?;
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
        self.audit_mutation_result(
            principal,
            AuthorizationAction::CancelRun,
            Some(key),
            None,
            Ok(()),
        )
        .await
    }

    pub async fn read_events(
        &self,
        principal: &VerifiedPrincipal,
        key: RunKey,
        after: Option<EventSequence>,
        limit: u16,
    ) -> Result<Vec<ServiceEventV2>, ServiceError> {
        validate_read_limit(limit)?;
        let authorization = self.run_authorization(key).await?;
        self.authorize(
            principal,
            AuthorizationAction::ReadEvents,
            AuthorizationResource::OwnedRun {
                workflow_id: authorization.workflow_id(),
                owner: authorization.owner(),
            },
        )?;
        let workflow_id = self.read.find_run(key).await?.and_then(|run| {
            self.recovery_workflows.iter().find_map(|(contract, id)| {
                (*contract == run.recovery_contract()).then_some(id.clone())
            })
        });
        Ok(self
            .read
            .read_events(key, after, limit)
            .await?
            .events()
            .iter()
            .map(|event| ServiceEventV2::new(event, workflow_id.clone()))
            .collect())
    }

    pub async fn run_page(
        &self,
        principal: &VerifiedPrincipal,
        after: Option<RunPageCursor>,
        limit: u16,
    ) -> Result<DurableRunPage, ServiceError> {
        validate_read_limit(limit)?;
        self.authorize(
            principal,
            AuthorizationAction::ReadOperations,
            AuthorizationResource::Global,
        )?;
        self.read.list_runs(after, limit).await.map_err(Into::into)
    }

    pub async fn operational_run_page(
        &self,
        principal: &VerifiedPrincipal,
        after: Option<RunPageCursor>,
        limit: u16,
        reconciliation_only: bool,
    ) -> Result<OperationalRunPageV1, ServiceError> {
        validate_read_limit(limit)?;
        self.authorize(
            principal,
            if reconciliation_only {
                AuthorizationAction::InspectReconciliation
            } else {
                AuthorizationAction::ReadOperations
            },
            AuthorizationResource::Global,
        )?;
        let page = self.read.list_runs(after, limit).await?;
        let mut items = Vec::new();
        for summary in page.items() {
            let mut view = self
                .view_for_disposition(self.harness.recover_run(summary.key()).await?)
                .await?;
            project_legacy_unowned(summary.authorization().is_none(), &mut view);
            if reconciliation_only
                && view.disposition != RunDispositionV1::ManualReconciliationRequired
            {
                continue;
            }
            items.push(OperationalRunViewV1 {
                session_id: view.session_id,
                run_id: view.run_id,
                disposition: view.disposition,
                last_sequence: view.last_sequence,
                workflow_id: view.workflow_id,
                duration_millis: view.duration_millis,
            });
        }
        Ok(OperationalRunPageV1 {
            items,
            next_run_id: page.next().map(RunPageCursor::run_id),
        })
    }

    pub async fn waiting_page(
        &self,
        principal: &VerifiedPrincipal,
        after: Option<WaitingPageCursor>,
        limit: u16,
    ) -> Result<(Vec<WaitingApprovalView>, Option<WaitingPageCursor>), ServiceError> {
        validate_read_limit(limit)?;
        self.authorize(
            principal,
            AuthorizationAction::ListWaiting,
            AuthorizationResource::Global,
        )?;
        let page: DurableWaitingPage = self.read.list_waiting(after, limit).await?;
        let mut items = Vec::new();
        for item in page.items() {
            let Ok(authorization) = self.run_authorization(item.key()).await else {
                continue;
            };
            if self.authorization.authorize(
                principal,
                AuthorizationAction::ReadApprovalPreview,
                AuthorizationResource::Approval {
                    workflow_id: authorization.workflow_id(),
                    owner: authorization.owner(),
                    requester: authorization.requester(),
                },
            ) != AuthorizationDecision::Granted
            {
                continue;
            }
            items.push(WaitingApprovalView {
                session_id: item.key().session_id(),
                run_id: item.key().run_id(),
                wait_id: item.wait_id(),
                row_version: item.row_version(),
                state: waiting_state(item.status()),
            });
        }
        Ok((items, page.next()))
    }

    pub async fn approval_preview(
        &self,
        principal: &VerifiedPrincipal,
        key: RunKey,
        wait_id: DurableApprovalWaitId,
    ) -> Result<ApprovalPreviewV1, ServiceError> {
        let authorization = self.run_authorization(key).await?;
        self.authorize(
            principal,
            AuthorizationAction::ReadApprovalPreview,
            AuthorizationResource::Approval {
                workflow_id: authorization.workflow_id(),
                owner: authorization.owner(),
                requester: authorization.requester(),
            },
        )?;
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
        principal: &VerifiedPrincipal,
        key: RunKey,
        wait_id: DurableApprovalWaitId,
        expected_row_version: u64,
        decision: ApprovalDecisionV1,
    ) -> Result<(), ServiceError> {
        self.ensure_serving()?;
        let authorization = self.run_authorization(key).await?;
        self.authorize_mutation(
            principal,
            AuthorizationAction::DecideApproval,
            AuthorizationResource::Approval {
                workflow_id: authorization.workflow_id(),
                owner: authorization.owner(),
                requester: authorization.requester(),
            },
            Some(key),
            Some(wait_id),
        )
        .await?;
        let outcome = match decision {
            ApprovalDecisionV1::Approve => DurableApprovalOutcome::Approve,
            ApprovalDecisionV1::Deny => DurableApprovalOutcome::Deny,
        };
        let decided_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| ServiceError::Conflict)?
            .as_millis()
            .try_into()
            .map_err(|_| ServiceError::Conflict)?;
        let result = self
            .harness
            .record_durable_approval_outcome_as(
                key,
                wait_id,
                expected_row_version,
                outcome,
                principal.id().clone(),
                decided_at,
            )
            .await
            .map_err(Into::into);
        self.audit_mutation_result(
            principal,
            AuthorizationAction::DecideApproval,
            Some(key),
            Some(wait_id),
            result,
        )
        .await
    }

    pub async fn abort_waiting(
        &self,
        principal: &VerifiedPrincipal,
        key: RunKey,
        wait_id: DurableApprovalWaitId,
        expected_row_version: u64,
    ) -> Result<(), ServiceError> {
        self.ensure_serving()?;
        let authorization = self.run_authorization(key).await?;
        self.authorize_mutation(
            principal,
            AuthorizationAction::AbortWaiting,
            AuthorizationResource::OwnedRun {
                workflow_id: authorization.workflow_id(),
                owner: authorization.owner(),
            },
            Some(key),
            Some(wait_id),
        )
        .await?;
        let result = self
            .harness
            .abort_durable_waiting(key, wait_id, expected_row_version)
            .await
            .map_err(Into::into);
        self.audit_mutation_result(
            principal,
            AuthorizationAction::AbortWaiting,
            Some(key),
            Some(wait_id),
            result,
        )
        .await
    }

    pub async fn resume_run(
        self: &Arc<Self>,
        principal: &VerifiedPrincipal,
        key: RunKey,
        wait_id: Option<DurableApprovalWaitId>,
    ) -> Result<(), ServiceError> {
        self.ensure_serving()?;
        let authorization = self.run_authorization(key).await?;
        self.authorize_mutation(
            principal,
            AuthorizationAction::ResumeRun,
            AuthorizationResource::OwnedRun {
                workflow_id: authorization.workflow_id(),
                owner: authorization.owner(),
            },
            Some(key),
            wait_id,
        )
        .await?;
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
            self.ensure_serving()?;
            if active.contains_key(&key.session_id()) || active.len() >= MAX_ACTIVE_RUNS {
                return Err(ServiceError::Conflict);
            }
            active.insert(
                key.session_id(),
                ActiveRun {
                    key,
                    disposition: RunDispositionV1::Running,
                    cancellation: Some(cancellation),
                    workflow_id: workflow_id.clone(),
                    started_at: Instant::now(),
                },
            );
        }
        self.observer.run_started();
        let service = self.clone();
        let harness = self.harness.clone();
        self.tasks.spawn(async move {
            let started = Instant::now();
            let completion = match disposition {
                RecoveryDisposition::Waiting(recovered) => {
                    if let Some(wait_id) = wait_id {
                        workflow
                            .resume_waiting(harness, *recovered, wait_id)
                            .await
                            .map(Some)
                    } else {
                        Ok(None)
                    }
                }
                RecoveryDisposition::Resumable(recovered) => workflow
                    .resume_recovered(harness, *recovered)
                    .await
                    .map(Some),
                _ => Ok(None),
            };
            if let Ok(Some(WorkflowCompletion::ApplicationResult(result))) = completion.as_ref() {
                service.store_result(key, result.clone()).await;
            }
            let disposition = service
                .harness
                .recover_run(key)
                .await
                .ok()
                .map(|value| disposition_kind(&value));
            service.observer.run_settled(disposition, started.elapsed());
            service.active.lock().await.remove(&key.session_id());
        });
        self.audit_mutation_result(
            principal,
            AuthorizationAction::ResumeRun,
            Some(key),
            wait_id,
            Ok(()),
        )
        .await
    }

    async fn store_result(&self, key: RunKey, result: ApplicationResultV1) {
        let duration_millis = self
            .active
            .lock()
            .await
            .get(&key.session_id())
            .filter(|active| active.key == key)
            .map(|active| {
                u64::try_from(active.started_at.elapsed().as_millis()).unwrap_or(u64::MAX)
            })
            .unwrap_or_default();
        let mut results = self.results.lock().await;
        if results.len() >= MAX_APPLICATION_RESULTS && !results.contains_key(&key) {
            results.clear();
        }
        results.insert(
            key,
            StoredApplicationResult {
                result,
                duration_millis,
            },
        );
    }

    pub async fn discover_runs(&self) -> Result<Vec<RunView>, ServiceError> {
        let mut cursor = None;
        let mut result = Vec::new();
        loop {
            let page = self.read.list_runs(cursor, 256).await?;
            for item in page.items() {
                let mut view = self
                    .view_for_disposition(self.harness.recover_run(item.key()).await?)
                    .await?;
                project_legacy_unowned(item.authorization().is_none(), &mut view);
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

    async fn view_for_disposition(
        &self,
        disposition: RecoveryDisposition,
    ) -> Result<RunView, ServiceError> {
        let contract = recovery_contract(&disposition);
        let mut view = disposition_view(disposition)?;
        view.workflow_id = contract.and_then(|contract| {
            self.recovery_workflows
                .iter()
                .find_map(|(candidate, id)| (*candidate == contract).then_some(id.clone()))
        });
        if let Some(stored) = self
            .results
            .lock()
            .await
            .get(&RunKey::new(view.run_id, view.session_id))
            .cloned()
        {
            view.result = Some(stored.result);
            view.duration_millis = Some(stored.duration_millis);
        }
        Ok(view)
    }

    async fn history_item(
        &self,
        summary: &DurableRunSummary,
    ) -> Result<RunHistoryItemV1, ServiceError> {
        let key = summary.key();
        let active = {
            let runs = self.active.lock().await;
            runs.get(&key.session_id())
                .filter(|active| active.key == key)
                .map(active_view)
        };
        let view = match active {
            Some(view) => view,
            None => {
                self.view_for_disposition(self.harness.recover_run(key).await?)
                    .await?
            }
        };
        Ok(RunHistoryItemV1 {
            run_id: view.run_id,
            disposition: view.disposition,
            last_sequence: view.last_sequence,
            outcome: view.outcome,
            workflow_id: view.workflow_id,
            started_at_unix_millis: summary.started_at_unix_millis(),
            result_available: view.result.is_some(),
        })
    }
}

fn conversation_title(session_id: SessionId) -> String {
    let value = session_id.to_string();
    format!("Conversation {}", &value[..8])
}

fn recovery_contract(disposition: &RecoveryDisposition) -> Option<RecoveryContract> {
    match disposition {
        RecoveryDisposition::Completed { state }
        | RecoveryDisposition::TerminalFailure { state, .. }
        | RecoveryDisposition::ManualReconciliationRequired { state, .. } => {
            Some(state.recovery_contract())
        }
        RecoveryDisposition::Resumable(run) => Some(run.recovery_contract()),
        RecoveryDisposition::Waiting(run) => Some(run.recovery_contract()),
    }
}

fn disposition_kind(disposition: &RecoveryDisposition) -> RunDispositionV1 {
    match disposition {
        RecoveryDisposition::Completed { .. } => RunDispositionV1::Completed,
        RecoveryDisposition::TerminalFailure { .. } => RunDispositionV1::Failed,
        RecoveryDisposition::Resumable(_) => RunDispositionV1::Resumable,
        RecoveryDisposition::Waiting(_) => RunDispositionV1::Waiting,
        RecoveryDisposition::ManualReconciliationRequired { .. } => {
            RunDispositionV1::ManualReconciliationRequired
        }
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

fn project_legacy_unowned(unowned: bool, view: &mut RunView) {
    if unowned && view.disposition == RunDispositionV1::Waiting {
        view.disposition = RunDispositionV1::ManualReconciliationRequired;
    }
}

fn active_view(active: &ActiveRun) -> RunView {
    RunView {
        session_id: active.key.session_id(),
        run_id: active.key.run_id(),
        disposition: active.disposition,
        last_sequence: None,
        outcome: None,
        workflow_id: Some(active.workflow_id.clone()),
        result: None,
        duration_millis: None,
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
        workflow_id: None,
        result: None,
        duration_millis: None,
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

pub type ServiceEventCategoryV2 = ServiceEventCategoryV1;
pub type ServiceEventPhaseV2 = ServiceEventPhaseV1;

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ServiceEventV2 {
    pub version: u16,
    pub sequence: u64,
    pub run_id: RunId,
    pub category: ServiceEventCategoryV1,
    pub phase: ServiceEventPhaseV1,
    pub correlation_id: Option<String>,
    pub workflow_id: Option<WorkflowId>,
    pub knowledge_backends: Vec<agent_core::KnowledgeBackendId>,
    pub graph_node_id: Option<agent_core::GraphNodeId>,
    pub budget_usage: Option<u32>,
    pub budget_limit: Option<u32>,
}

impl ServiceEventV2 {
    fn new(event: &AgentEvent, workflow_id: Option<WorkflowId>) -> Self {
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
        let knowledge_backends = match event.kind() {
            AgentEventKind::KnowledgeRetrievalStarted { route, .. }
            | AgentEventKind::KnowledgeRetrievalRestarted { route, .. } => {
                route.backends().to_vec()
            }
            _ => Vec::new(),
        };
        let graph_node_id = match event.kind() {
            AgentEventKind::Graph { event } => match event {
                agent_core::GraphProgressEvent::GraphStarted { start_node, .. } => {
                    Some(start_node.clone())
                }
                agent_core::GraphProgressEvent::GraphNodeEntered { node_id, .. }
                | agent_core::GraphProgressEvent::GraphNodeRestarted { node_id, .. }
                | agent_core::GraphProgressEvent::GraphNodeCompleted { node_id, .. }
                | agent_core::GraphProgressEvent::GraphSuspended { node_id, .. }
                | agent_core::GraphProgressEvent::GraphResumed { node_id, .. }
                | agent_core::GraphProgressEvent::GraphCompleted { node_id, .. }
                | agent_core::GraphProgressEvent::GraphFailed { node_id, .. } => {
                    Some(node_id.clone())
                }
            },
            _ => None,
        };
        let (budget_usage, budget_limit) = match event.kind() {
            AgentEventKind::ModelInvocationStarted { usage, limit, .. }
            | AgentEventKind::ToolInvocationStarted { usage, limit, .. } => {
                (Some(*usage), Some(*limit))
            }
            AgentEventKind::Graph {
                event:
                    agent_core::GraphProgressEvent::GraphNodeEntered { step, limit, .. }
                    | agent_core::GraphProgressEvent::GraphNodeRestarted { step, limit, .. },
            } => (Some(*step), Some(*limit)),
            _ => (None, None),
        };
        Self {
            version: SERVICE_EVENT_VERSION,
            sequence: event.sequence().get(),
            run_id: event.run_id(),
            category,
            phase,
            correlation_id: id,
            workflow_id,
            knowledge_backends,
            graph_node_id,
            budget_usage,
            budget_limit,
        }
    }
}

#[derive(Debug, Error)]
pub enum ServiceError {
    #[error("invalid request")]
    InvalidRequest,
    #[error("operation is not authorized")]
    Unauthorized,
    #[error("resource not found")]
    NotFound,
    #[error("request conflicts with current state")]
    Conflict,
    #[error("service capacity exhausted")]
    Capacity,
    #[error("service is draining")]
    Draining,
    #[error("workflow unavailable")]
    WorkflowUnavailable,
    #[error("service configuration is invalid")]
    Configuration,
    #[error("durable state predates principal ownership")]
    LegacyUnowned,
    #[error("required security audit failed")]
    SecurityAudit,
    #[error("durable identity state failed")]
    IdentityStore(#[from] agent_identity::IdentityStoreError),
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
    use agent_identity::{
        ApprovalSeparation, PrincipalRole,
        testing::{InMemorySecurityAudit, policy, principal},
    };
    use agent_persistence_sqlite::SqliteRunPersistence;

    use super::*;

    fn test_principal() -> VerifiedPrincipal {
        principal("test-user", &[PrincipalRole::User, PrincipalRole::Approver])
    }

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
        fn budget(&self) -> RunBudget {
            RunBudget::new(1, 1, 1, Duration::from_secs(10)).expect("budget")
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
        ) -> ServiceFuture<'static, Result<WorkflowCompletion, WorkflowError>> {
            Box::pin(async move {
                self.invocations.fetch_add(1, Ordering::SeqCst);
                tokio::time::sleep(self.delay).await;
                match harness.checkpoint(&mut context).await {
                    Ok(()) => harness
                        .complete_run(&mut context)
                        .await
                        .map(|()| WorkflowCompletion::NoApplicationResult)
                        .map_err(|_| WorkflowError::Failed),
                    Err(_) => Ok(WorkflowCompletion::NoApplicationResult),
                }
            })
        }
        fn resume_waiting(
            self: Arc<Self>,
            _harness: Arc<ExecutionHarness>,
            _recovered: RecoveredWaitingRun,
            _wait_id: DurableApprovalWaitId,
        ) -> ServiceFuture<'static, Result<WorkflowCompletion, WorkflowError>> {
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
        let authorization = Arc::new(policy(
            ["test".to_owned()],
            ApprovalSeparation::RequesterMayApprove,
        ));
        let service = Arc::new(
            AgentService::new(
                harness,
                store.clone(),
                store,
                authorization,
                Arc::new(InMemorySecurityAudit::default()),
                vec![workflow],
            )
            .expect("service"),
        );
        (directory, service, invocations)
    }

    #[tokio::test]
    async fn duplicate_start_is_idempotent_and_distinct_concurrent_start_conflicts() {
        let (_directory, service, invocations) = fixture(Duration::from_millis(100)).await;
        let principal = test_principal();
        let session = service.create_session(&principal).await.expect("session");
        let request = Uuid::new_v4();
        let workflow = WorkflowId::new("test").expect("workflow");
        let first = service.start_run(
            &principal,
            session,
            request,
            &workflow,
            RunInput::new(b"one".to_vec()).expect("input"),
        );
        let second = service.start_run(
            &principal,
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
                    &test_principal(),
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
            .get_run_status(&test_principal(), RunKey::new(first.run_id, session))
            .await
            .expect("status");
        assert_eq!(status.disposition, RunDispositionV1::Completed);
        let retry = service
            .start_run(
                &test_principal(),
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
    async fn runtime_status_is_bounded_sorted_and_contains_no_runtime_authority() {
        let (_directory, service, _invocations) = fixture(Duration::from_millis(1)).await;
        let status = service
            .runtime_status(&test_principal())
            .expect("runtime status");

        assert_eq!(status.principal.principal_id.as_str(), "test-user");
        assert_eq!(
            status.principal.roles,
            vec![PrincipalRole::User, PrincipalRole::Approver]
        );
        assert_eq!(status.max_active_runs as usize, MAX_ACTIVE_RUNS);
        assert_eq!(status.max_run_input_bytes as usize, MAX_RUN_INPUT_BYTES);
        assert_eq!(status.workflow_budgets.len(), 1);
        assert_eq!(status.workflow_budgets[0].workflow_id.as_str(), "test");
        assert_eq!(status.workflow_budgets[0].max_model_calls, 1);

        let encoded = serde_json::to_string(&status).expect("serialize status");
        for forbidden in [
            "credential",
            "token",
            "endpoint",
            "capsule",
            "action",
            "evidence",
        ] {
            assert!(!encoded.contains(forbidden), "status leaked {forbidden}");
        }
    }

    #[tokio::test]
    async fn cancellation_and_event_projection_remain_metadata_only() {
        let (_directory, service, _) = fixture(Duration::from_millis(100)).await;
        let session = service
            .create_session(&test_principal())
            .await
            .expect("session");
        let workflow = WorkflowId::new("test").expect("workflow");
        let run = service
            .start_run(
                &test_principal(),
                session,
                Uuid::new_v4(),
                &workflow,
                RunInput::new(b"secret-prompt".to_vec()).expect("input"),
            )
            .await
            .expect("start");
        let key = RunKey::new(run.run_id, session);
        service
            .cancel_active_run(&test_principal(), key)
            .await
            .expect("cancel");
        tokio::time::sleep(Duration::from_millis(150)).await;
        let events = service
            .read_events(&test_principal(), key, None, 64)
            .await
            .expect("events");
        let encoded = serde_json::to_string(&events).expect("encode");
        assert!(!encoded.contains("secret-prompt"));
        assert!(events.iter().all(|event| event.version == 2));
        assert!(
            events
                .iter()
                .all(|event| event.workflow_id.as_ref() == Some(&workflow))
        );
        assert_eq!(events.first().map(|event| event.sequence), Some(0));
        assert!(
            events
                .windows(2)
                .all(|events| events[1].sequence == events[0].sequence + 1)
        );
        assert_eq!(
            service
                .get_run_status(&test_principal(), key)
                .await
                .expect("status")
                .disposition,
            RunDispositionV1::Failed
        );
    }

    #[tokio::test]
    async fn draining_rejects_new_work_and_waits_for_active_tasks() {
        let (_directory, service, invocations) = fixture(Duration::from_millis(50)).await;
        let session = service
            .create_session(&test_principal())
            .await
            .expect("session");
        let workflow = WorkflowId::new("test").expect("workflow");
        service
            .start_run(
                &test_principal(),
                session,
                Uuid::new_v4(),
                &workflow,
                RunInput::new(b"bounded".to_vec()).expect("input"),
            )
            .await
            .expect("start");
        let report = service.drain(Duration::from_secs(1)).await.expect("drain");
        assert_eq!(report.remaining_active_runs, 0);
        assert_eq!(service.lifecycle(), ServiceLifecycleV1::Draining);
        assert_eq!(invocations.load(Ordering::SeqCst), 1);
        assert!(matches!(
            service.create_session(&test_principal()).await,
            Err(ServiceError::Draining)
        ));
    }

    #[tokio::test]
    async fn restart_discovers_run_and_restores_its_session() {
        let (directory, first, _) = fixture(Duration::from_millis(1)).await;
        let session = first
            .create_session(&test_principal())
            .await
            .expect("session");
        let workflow_id = WorkflowId::new("test").expect("workflow");
        let run = first
            .start_run(
                &test_principal(),
                session,
                Uuid::new_v4(),
                &workflow_id,
                RunInput::new(b"first".to_vec()).expect("input"),
            )
            .await
            .expect("start");
        let key = RunKey::new(run.run_id, session);
        let mut completed = false;
        for _ in 0..100 {
            if first
                .get_run_status(&test_principal(), key)
                .await
                .is_ok_and(|view| view.disposition == RunDispositionV1::Completed)
            {
                completed = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(completed, "run must complete before service reconstruction");

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
        let second = Arc::new(
            AgentService::new(
                harness,
                store.clone(),
                store,
                Arc::new(policy(
                    ["test".to_owned()],
                    ApprovalSeparation::RequesterMayApprove,
                )),
                Arc::new(InMemorySecurityAudit::default()),
                vec![workflow],
            )
            .expect("restarted service"),
        );
        let discovered = second.discover_runs().await.expect("discover");
        assert_eq!(discovered.len(), 1);
        assert_eq!(discovered[0].disposition, RunDispositionV1::Completed);
        second
            .start_run(
                &test_principal(),
                session,
                Uuid::new_v4(),
                &workflow_id,
                RunInput::new(b"second".to_vec()).expect("input"),
            )
            .await
            .expect("recovered session accepts new run");
    }

    #[tokio::test]
    async fn durable_owner_blocks_cross_principal_run_access() {
        let (_directory, service, _) = fixture(Duration::from_millis(20)).await;
        let owner = test_principal();
        let stranger = principal("other-user", &[PrincipalRole::User]);
        let session = service.create_session(&owner).await.expect("session");
        let run = service
            .start_run(
                &owner,
                session,
                Uuid::new_v4(),
                &WorkflowId::new("test").expect("workflow"),
                RunInput::new(b"owned".to_vec()).expect("input"),
            )
            .await
            .expect("start");
        let key = RunKey::new(run.run_id, session);

        assert!(matches!(
            service.get_run_status(&stranger, key).await,
            Err(ServiceError::Unauthorized)
        ));
        assert!(matches!(
            service.read_events(&stranger, key, None, 16).await,
            Err(ServiceError::Unauthorized)
        ));
        assert!(matches!(
            service.session_run_page(&stranger, session, None, 16).await,
            Err(ServiceError::Unauthorized)
        ));
    }

    #[tokio::test]
    async fn conversation_history_is_owner_scoped_paginated_and_restart_safe() {
        let (directory, service, _) = fixture(Duration::from_millis(20)).await;
        let owner = test_principal();
        let stranger = principal("other-user", &[PrincipalRole::User]);
        let first = service.create_session(&owner).await.expect("first session");
        let second = service
            .create_session(&owner)
            .await
            .expect("second session");
        let hidden = service
            .create_session(&stranger)
            .await
            .expect("other session");
        let run = service
            .start_run(
                &owner,
                first,
                Uuid::new_v4(),
                &WorkflowId::new("test").expect("workflow"),
                RunInput::new(b"history".to_vec()).expect("input"),
            )
            .await
            .expect("start");

        let page = service
            .conversation_page(&owner, None, 1)
            .await
            .expect("first page");
        assert_eq!(page.items.len(), 1);
        assert_eq!(page.items[0].session_id, second);
        let cursor = page.next_session_id.expect("second owner session");
        let tail = service
            .conversation_page(&owner, Some(SessionPageCursor::new(cursor)), 1)
            .await
            .expect("second page");
        assert_eq!(tail.items.len(), 1);
        assert_eq!(tail.items[0].session_id, first);
        assert_eq!(tail.next_session_id, None);
        assert!(
            service
                .conversation_page(&owner, None, 8)
                .await
                .expect("owner page")
                .items
                .iter()
                .all(|item| item.session_id != hidden)
        );

        let runs = service
            .session_run_page(&owner, first, None, 8)
            .await
            .expect("run history");
        assert_eq!(runs.items.len(), 1);
        assert_eq!(runs.items[0].run_id, run.run_id);
        assert_eq!(runs.next_run_id, None);

        drop(service);
        let reopened_store = Arc::new(
            SqliteRunPersistence::open(directory.path().join("runs.sqlite3"))
                .await
                .expect("reopen store"),
        );
        assert_eq!(
            reopened_store
                .list_sessions(owner.id(), None, 8)
                .await
                .expect("durable sessions")
                .items()
                .len(),
            2
        );
    }

    #[tokio::test]
    async fn required_security_audit_failure_prevents_session_mutation() {
        let directory = tempfile::tempdir().expect("directory");
        let store = Arc::new(
            SqliteRunPersistence::open(directory.path().join("runs.sqlite3"))
                .await
                .expect("store"),
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
            id: WorkflowId::new("test").expect("workflow"),
            invocations: Arc::new(AtomicUsize::new(0)),
            delay: Duration::from_millis(1),
        });
        let service = AgentService::new(
            harness,
            store.clone(),
            store,
            Arc::new(policy(
                ["test".to_owned()],
                ApprovalSeparation::RequesterMayApprove,
            )),
            Arc::new(InMemorySecurityAudit::failing()),
            vec![workflow],
        )
        .expect("service");

        assert!(matches!(
            service.create_session(&test_principal()).await,
            Err(ServiceError::SecurityAudit)
        ));
        assert!(service.sessions.lock().await.is_empty());
    }

    #[tokio::test]
    async fn security_audit_orders_authorization_before_durable_mutation_commit() {
        let directory = tempfile::tempdir().expect("directory");
        let store = Arc::new(
            SqliteRunPersistence::open(directory.path().join("runs.sqlite3"))
                .await
                .expect("store"),
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
            id: WorkflowId::new("test").expect("workflow"),
            invocations: Arc::new(AtomicUsize::new(0)),
            delay: Duration::from_millis(1),
        });
        let security_audit = Arc::new(InMemorySecurityAudit::default());
        let service = AgentService::new(
            harness,
            store.clone(),
            store.clone(),
            Arc::new(policy(
                ["test".to_owned()],
                ApprovalSeparation::RequesterMayApprove,
            )),
            security_audit.clone(),
            vec![workflow],
        )
        .expect("service");
        let actor = test_principal();

        let session_id = service.create_session(&actor).await.expect("session");
        assert_eq!(
            security_audit
                .events()
                .iter()
                .map(|event| event.phase)
                .collect::<Vec<_>>(),
            [
                SecurityAuditPhase::AuthorizationGranted,
                SecurityAuditPhase::MutationRequested,
                SecurityAuditPhase::MutationCommitted,
            ]
        );
        assert_eq!(
            store
                .load_session(session_id)
                .await
                .expect("ownership")
                .expect("durable session")
                .owner(),
            actor.id()
        );
    }

    #[test]
    fn legacy_unowned_waiting_is_never_exposed_as_resumable_waiting() {
        let mut view = RunView {
            session_id: SessionId::new(),
            run_id: RunId::new(),
            disposition: RunDispositionV1::Waiting,
            last_sequence: None,
            outcome: None,
            workflow_id: None,
            result: None,
            duration_millis: None,
        };

        project_legacy_unowned(true, &mut view);
        assert_eq!(
            view.disposition,
            RunDispositionV1::ManualReconciliationRequired
        );
    }
}
