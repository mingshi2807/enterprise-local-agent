//! Local HTTP/JSON and SSE transport for `agent-service`.

use std::{
    collections::{HashMap, VecDeque},
    convert::Infallible,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use agent_deployment::{
    BuildInfoV1, OperationalMetrics, OperationsState, ReadinessCache, ReadinessSnapshotV1,
    ReadinessStatusV1,
};
use agent_service::{
    AgentService, ApprovalDecisionV1, DurableApprovalWaitId, EventSequence, MAX_ACTIVE_RUNS,
    MAX_COMMAND_QUEUE, OperationalRunPageV1, OperationalRunViewV1, RunId, RunInput, RunKey,
    ServiceError, ServiceEventV2, SessionId, WaitingPageCursor, WorkflowId,
};
use axum::{
    Json, Router,
    body::Body,
    extract::{DefaultBodyLimit, Path, Query, State},
    http::{HeaderMap, Request, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response, Sse, sse::Event},
    routing::{get, post},
};
use futures_util::stream;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use uuid::Uuid;

pub const MAX_REQUEST_BODY_BYTES: usize = 16 * 1024;
pub const MAX_SSE_CONNECTIONS: usize = 64;
pub const MAX_SSE_CONNECTIONS_PER_RUN: usize = 4;
const SSE_PAGE_SIZE: u16 = 64;
const SSE_POLL_INTERVAL: Duration = Duration::from_millis(100);
const SSE_MAX_LIFETIME: Duration = Duration::from_secs(300);

#[derive(Clone)]
pub struct HttpSecurity {
    bearer_digest: Option<[u8; 32]>,
    allowed_hosts: Arc<[String]>,
    allowed_origins: Arc<[String]>,
}

impl HttpSecurity {
    #[must_use]
    pub fn unix_socket() -> Self {
        Self {
            bearer_digest: None,
            allowed_hosts: Arc::from([]),
            allowed_origins: Arc::from([]),
        }
    }

    pub fn loopback(
        bearer: &str,
        allowed_hosts: Vec<String>,
        allowed_origins: Vec<String>,
    ) -> Result<Self, HttpConfigurationError> {
        if bearer.len() < 32
            || allowed_hosts.is_empty()
            || allowed_origins.is_empty()
            || allowed_hosts.iter().any(|value| value == "*")
            || allowed_origins.iter().any(|value| value == "*")
        {
            return Err(HttpConfigurationError);
        }
        Ok(Self {
            bearer_digest: Some(Sha256::digest(bearer.as_bytes()).into()),
            allowed_hosts: allowed_hosts.into(),
            allowed_origins: allowed_origins.into(),
        })
    }
}

#[derive(Clone)]
struct HttpState {
    service: Arc<AgentService>,
    security: HttpSecurity,
    streams: StreamLimits,
    commands: Arc<Semaphore>,
    operations: OperationsState,
}

impl HttpState {
    fn acquire_command(&self) -> Result<OwnedSemaphorePermit, ApiError> {
        self.commands
            .clone()
            .try_acquire_owned()
            .map_err(|_| ApiError::Capacity)
    }
}

#[derive(Clone)]
struct StreamLimits {
    global: Arc<Semaphore>,
    per_run: Arc<Mutex<HashMap<RunKey, Arc<Semaphore>>>>,
}

impl StreamLimits {
    fn new() -> Self {
        Self {
            global: Arc::new(Semaphore::new(MAX_SSE_CONNECTIONS)),
            per_run: Arc::new(Mutex::new(HashMap::with_capacity(MAX_ACTIVE_RUNS))),
        }
    }

    async fn acquire(&self, key: RunKey) -> Result<StreamPermit, ApiError> {
        let global = self
            .global
            .clone()
            .try_acquire_owned()
            .map_err(|_| ApiError::Capacity)?;
        let semaphore = self
            .per_run
            .lock()
            .map_err(|_| ApiError::Internal)?
            .entry(key)
            .or_insert_with(|| Arc::new(Semaphore::new(MAX_SSE_CONNECTIONS_PER_RUN)))
            .clone();
        let per_run = semaphore
            .try_acquire_owned()
            .map_err(|_| ApiError::Capacity)?;
        Ok(StreamPermit {
            _global: global,
            _per_run: per_run,
        })
    }
}

struct StreamPermit {
    _global: OwnedSemaphorePermit,
    _per_run: OwnedSemaphorePermit,
}

#[derive(Clone, Copy, Debug)]
pub struct HttpConfigurationError;

pub fn router(service: Arc<AgentService>, security: HttpSecurity) -> Router {
    let operations = OperationsState::new(
        ReadinessCache::new(ReadinessSnapshotV1 {
            version: 1,
            overall: ReadinessStatusV1::Unavailable,
            dependencies: vec![],
            workflows: vec![],
        }),
        OperationalMetrics::new(),
        BuildInfoV1 {
            application: "enterprise-local-agent".to_owned(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
            git_identity: None,
            deployment_fingerprint: "unconfigured".to_owned(),
            config_schema_version: 1,
            store_schema_version: agent_harness::CURRENT_STORE_SCHEMA_VERSION,
            event_schema_version: agent_core::CURRENT_EVENT_SCHEMA_VERSION.get(),
            checkpoint_schema_version: agent_harness::CURRENT_CHECKPOINT_SCHEMA_VERSION,
        },
    );
    router_with_operations(service, security, operations)
}

pub fn router_with_operations(
    service: Arc<AgentService>,
    security: HttpSecurity,
    operations: OperationsState,
) -> Router {
    let state = HttpState {
        service,
        security,
        streams: StreamLimits::new(),
        commands: Arc::new(Semaphore::new(MAX_COMMAND_QUEUE)),
        operations,
    };
    Router::new()
        .route("/healthz", get(health))
        .route("/v1/operations/readiness", get(readiness))
        .route("/v1/operations/metrics", get(metrics))
        .route("/v1/operations/version", get(version))
        .route("/v1/operations/runs", get(operational_runs))
        .route(
            "/v1/operations/runs/{session_id}/{run_id}",
            get(operational_run),
        )
        .route("/v1/operations/reconciliation", get(reconciliation_runs))
        .route("/v1/sessions", post(create_session))
        .route("/v1/sessions/{session_id}/runs", post(start_run))
        .route("/v1/sessions/{session_id}/runs/{run_id}", get(run_status))
        .route(
            "/v1/sessions/{session_id}/runs/{run_id}/events",
            get(read_events),
        )
        .route(
            "/v1/sessions/{session_id}/runs/{run_id}/events/stream",
            get(stream_events),
        )
        .route("/v1/approvals", get(list_waiting))
        .route(
            "/v1/sessions/{session_id}/runs/{run_id}/approvals/{wait_id}/preview",
            get(approval_preview),
        )
        .route(
            "/v1/sessions/{session_id}/runs/{run_id}/approvals/{wait_id}/decision",
            post(submit_decision),
        )
        .route(
            "/v1/sessions/{session_id}/runs/{run_id}/resume",
            post(resume_run),
        )
        .route(
            "/v1/sessions/{session_id}/runs/{run_id}/cancel",
            post(cancel_run),
        )
        .route(
            "/v1/sessions/{session_id}/runs/{run_id}/approvals/{wait_id}/abort",
            post(abort_waiting),
        )
        .layer(DefaultBodyLimit::max(MAX_REQUEST_BODY_BYTES))
        .layer(middleware::from_fn_with_state(state.clone(), authorize))
        .with_state(state)
}

#[derive(Serialize)]
struct HealthResponse {
    version: u16,
    lifecycle: agent_service::ServiceLifecycleV1,
}

async fn health(State(state): State<HttpState>) -> Json<HealthResponse> {
    Json(HealthResponse {
        version: 1,
        lifecycle: state.service.lifecycle(),
    })
}

async fn readiness(State(state): State<HttpState>) -> Result<Json<ReadinessSnapshotV1>, ApiError> {
    state
        .operations
        .readiness_cache()
        .snapshot()
        .map(Json)
        .map_err(|_| ApiError::Internal)
}

async fn metrics(
    State(state): State<HttpState>,
) -> Result<Json<agent_deployment::OperationalMetricsSnapshotV1>, ApiError> {
    state
        .operations
        .metrics()
        .snapshot()
        .map(Json)
        .map_err(|_| ApiError::Internal)
}

async fn version(State(state): State<HttpState>) -> Json<BuildInfoV1> {
    Json(state.operations.build().clone())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OperationalRunsQuery {
    after_run_id: Option<RunId>,
    limit: Option<u16>,
}

async fn operational_runs(
    State(state): State<HttpState>,
    Query(query): Query<OperationalRunsQuery>,
) -> Result<Json<OperationalRunPageV1>, ApiError> {
    state
        .service
        .operational_run_page(
            query.after_run_id.map(agent_harness::RunPageCursor::new),
            query.limit.unwrap_or(64),
            false,
        )
        .await
        .map(Json)
        .map_err(Into::into)
}

async fn reconciliation_runs(
    State(state): State<HttpState>,
    Query(query): Query<OperationalRunsQuery>,
) -> Result<Json<OperationalRunPageV1>, ApiError> {
    state
        .service
        .operational_run_page(
            query.after_run_id.map(agent_harness::RunPageCursor::new),
            query.limit.unwrap_or(64),
            true,
        )
        .await
        .map(Json)
        .map_err(Into::into)
}

async fn operational_run(
    State(state): State<HttpState>,
    Path((session_id, run_id)): Path<(String, String)>,
) -> Result<Json<OperationalRunViewV1>, ApiError> {
    let view = state
        .service
        .get_run_status(key(&session_id, &run_id)?)
        .await?;
    Ok(Json(OperationalRunViewV1 {
        session_id: view.session_id,
        run_id: view.run_id,
        disposition: view.disposition,
        last_sequence: view.last_sequence,
        workflow_id: view.workflow_id,
        duration_millis: view.duration_millis,
    }))
}

async fn authorize(
    State(state): State<HttpState>,
    request: Request<Body>,
    next: Next,
) -> Result<Response, ApiError> {
    if let Some(expected) = state.security.bearer_digest {
        let supplied = request
            .headers()
            .get("authorization")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.strip_prefix("Bearer "))
            .map(|value| Sha256::digest(value.as_bytes()))
            .ok_or(ApiError::Unauthorized)?;
        if !constant_time_equal(&expected, supplied.as_slice()) {
            return Err(ApiError::Unauthorized);
        }
        require_allowed_header(request.headers(), "host", &state.security.allowed_hosts)?;
        require_allowed_header(request.headers(), "origin", &state.security.allowed_origins)?;
    }
    let _ = state.operations.metrics().connection_opened();
    let response = next.run(request).await;
    let _ = state.operations.metrics().connection_closed();
    Ok(response)
}

fn constant_time_equal(left: &[u8], right: &[u8]) -> bool {
    left.len() == right.len()
        && left
            .iter()
            .zip(right)
            .fold(0_u8, |difference, (a, b)| difference | (a ^ b))
            == 0
}

fn require_allowed_header(
    headers: &HeaderMap,
    name: &'static str,
    allowed: &[String],
) -> Result<(), ApiError> {
    let value = headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .ok_or(ApiError::Unauthorized)?;
    allowed
        .iter()
        .any(|item| item == value)
        .then_some(())
        .ok_or(ApiError::Unauthorized)
}

#[derive(Serialize)]
struct SessionResponse {
    session_id: SessionId,
}

async fn create_session(State(state): State<HttpState>) -> Result<Json<SessionResponse>, ApiError> {
    let _permit = state.acquire_command()?;
    Ok(Json(SessionResponse {
        session_id: state.service.create_session().await?,
    }))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StartRunBody {
    start_request_id: Uuid,
    workflow_id: WorkflowId,
    input: String,
}

async fn start_run(
    State(state): State<HttpState>,
    Path(session_id): Path<String>,
    Json(body): Json<StartRunBody>,
) -> Result<Json<agent_service::RunView>, ApiError> {
    let _permit = state.acquire_command()?;
    let session_id = parse_id(&session_id)?;
    body.workflow_id.validate()?;
    let input = RunInput::new(body.input.into_bytes())?;
    Ok(Json(
        state
            .service
            .start_run(session_id, body.start_request_id, &body.workflow_id, input)
            .await?,
    ))
}

fn key(session_id: &str, run_id: &str) -> Result<RunKey, ApiError> {
    Ok(RunKey::new(parse_id(run_id)?, parse_id(session_id)?))
}

async fn run_status(
    State(state): State<HttpState>,
    Path((session_id, run_id)): Path<(String, String)>,
) -> Result<Json<agent_service::RunView>, ApiError> {
    Ok(Json(
        state
            .service
            .get_run_status(key(&session_id, &run_id)?)
            .await?,
    ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EventQuery {
    after: Option<u64>,
    limit: Option<u16>,
}

async fn read_events(
    State(state): State<HttpState>,
    Path((session_id, run_id)): Path<(String, String)>,
    Query(query): Query<EventQuery>,
) -> Result<Json<Vec<ServiceEventV2>>, ApiError> {
    Ok(Json(
        state
            .service
            .read_events(
                key(&session_id, &run_id)?,
                query.after.map(EventSequence::new),
                query.limit.unwrap_or(64),
            )
            .await?,
    ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WaitingQuery {
    limit: Option<u16>,
    after_run_id: Option<RunId>,
    after_wait_id: Option<DurableApprovalWaitId>,
}

#[derive(Serialize)]
struct WaitingPageResponse {
    items: Vec<agent_service::WaitingApprovalView>,
    next_run_id: Option<RunId>,
    next_wait_id: Option<DurableApprovalWaitId>,
}

async fn list_waiting(
    State(state): State<HttpState>,
    Query(query): Query<WaitingQuery>,
) -> Result<Json<WaitingPageResponse>, ApiError> {
    let cursor = match (query.after_run_id, query.after_wait_id) {
        (None, None) => None,
        (Some(run_id), Some(wait_id)) => Some(WaitingPageCursor::new(run_id, wait_id)),
        _ => return Err(ApiError::InvalidRequest),
    };
    let (items, next) = state
        .service
        .waiting_page(cursor, query.limit.unwrap_or(64))
        .await?;
    Ok(Json(WaitingPageResponse {
        items,
        next_run_id: next.map(WaitingPageCursor::run_id),
        next_wait_id: next.map(WaitingPageCursor::wait_id),
    }))
}

async fn approval_preview(
    State(state): State<HttpState>,
    Path((session_id, run_id, wait_id)): Path<(String, String, String)>,
) -> Result<Json<agent_service::ApprovalPreviewV1>, ApiError> {
    Ok(Json(
        state
            .service
            .approval_preview(key(&session_id, &run_id)?, parse_id(&wait_id)?)
            .await?,
    ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DecisionBody {
    expected_row_version: u64,
    decision: ApprovalDecisionV1,
}

async fn submit_decision(
    State(state): State<HttpState>,
    Path((session_id, run_id, wait_id)): Path<(String, String, String)>,
    Json(body): Json<DecisionBody>,
) -> Result<StatusCode, ApiError> {
    let _permit = state.acquire_command()?;
    state
        .service
        .submit_decision(
            key(&session_id, &run_id)?,
            parse_id(&wait_id)?,
            body.expected_row_version,
            body.decision,
        )
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ResumeBody {
    wait_id: Option<DurableApprovalWaitId>,
}

async fn resume_run(
    State(state): State<HttpState>,
    Path((session_id, run_id)): Path<(String, String)>,
    Json(body): Json<ResumeBody>,
) -> Result<StatusCode, ApiError> {
    let _permit = state.acquire_command()?;
    state
        .service
        .resume_run(key(&session_id, &run_id)?, body.wait_id)
        .await?;
    Ok(StatusCode::ACCEPTED)
}

async fn cancel_run(
    State(state): State<HttpState>,
    Path((session_id, run_id)): Path<(String, String)>,
) -> Result<StatusCode, ApiError> {
    let _permit = state.acquire_command()?;
    state
        .service
        .cancel_active_run(key(&session_id, &run_id)?)
        .await?;
    Ok(StatusCode::ACCEPTED)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AbortBody {
    expected_row_version: u64,
}

async fn abort_waiting(
    State(state): State<HttpState>,
    Path((session_id, run_id, wait_id)): Path<(String, String, String)>,
    Json(body): Json<AbortBody>,
) -> Result<StatusCode, ApiError> {
    let _permit = state.acquire_command()?;
    state
        .service
        .abort_waiting(
            key(&session_id, &run_id)?,
            parse_id(&wait_id)?,
            body.expected_row_version,
        )
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

struct SseState {
    service: Arc<AgentService>,
    key: RunKey,
    cursor: Option<EventSequence>,
    pending: VecDeque<ServiceEventV2>,
    deadline: Instant,
    _permit: StreamPermit,
    done: bool,
}

async fn stream_events(
    State(state): State<HttpState>,
    Path((session_id, run_id)): Path<(String, String)>,
    Query(query): Query<EventQuery>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, ApiError> {
    let key = key(&session_id, &run_id)?;
    let permit = state.streams.acquire(key).await?;
    let header_cursor = headers
        .get("last-event-id")
        .map(|value| value.to_str().map_err(|_| ApiError::InvalidRequest))
        .transpose()?
        .map(|value| value.parse::<u64>().map_err(|_| ApiError::InvalidRequest))
        .transpose()?;
    if query.after.is_some() && header_cursor.is_some() && query.after != header_cursor {
        return Err(ApiError::InvalidRequest);
    }
    let after = query.after.or(header_cursor);
    let stream_state = SseState {
        service: state.service,
        key,
        cursor: after.map(EventSequence::new),
        pending: VecDeque::new(),
        deadline: Instant::now() + SSE_MAX_LIFETIME,
        _permit: permit,
        done: false,
    };
    let events = stream::unfold(stream_state, |mut state| async move {
        loop {
            if state.done || Instant::now() >= state.deadline {
                return None;
            }
            if let Some(event) = state.pending.pop_front() {
                state.cursor = Some(EventSequence::new(event.sequence));
                let encoded = match serde_json::to_string(&event) {
                    Ok(encoded) => encoded,
                    Err(_) => {
                        state.done = true;
                        return Some((
                            Ok::<_, Infallible>(
                                Event::default().event("error").data("encoding_failed"),
                            ),
                            state,
                        ));
                    }
                };
                return Some((
                    Ok::<_, Infallible>(
                        Event::default()
                            .id(event.sequence.to_string())
                            .event("metadata")
                            .data(encoded),
                    ),
                    state,
                ));
            }
            match state
                .service
                .read_events(state.key, state.cursor, SSE_PAGE_SIZE)
                .await
            {
                Ok(events) if events.is_empty() => tokio::time::sleep(SSE_POLL_INTERVAL).await,
                Ok(events) => state.pending.extend(events),
                Err(_) => {
                    state.done = true;
                    return Some((
                        Ok::<_, Infallible>(
                            Event::default().event("error").data("replay_boundary"),
                        ),
                        state,
                    ));
                }
            }
        }
    });
    Ok(Sse::new(events)
        .keep_alive(axum::response::sse::KeepAlive::new().interval(Duration::from_secs(15))))
}

fn parse_id<T: std::str::FromStr>(value: &str) -> Result<T, ApiError> {
    value.parse().map_err(|_| ApiError::InvalidRequest)
}

#[derive(Debug)]
enum ApiError {
    InvalidRequest,
    Unauthorized,
    NotFound,
    Conflict,
    Capacity,
    Unavailable,
    Internal,
}

impl From<ServiceError> for ApiError {
    fn from(error: ServiceError) -> Self {
        match error {
            ServiceError::InvalidRequest => Self::InvalidRequest,
            ServiceError::NotFound => Self::NotFound,
            ServiceError::Conflict => Self::Conflict,
            ServiceError::Capacity => Self::Capacity,
            ServiceError::Draining => Self::Unavailable,
            ServiceError::WorkflowUnavailable => Self::NotFound,
            ServiceError::Configuration
            | ServiceError::Harness(_)
            | ServiceError::Persistence(_)
            | ServiceError::Context(_) => Self::Internal,
        }
    }
}

#[derive(Serialize)]
struct ErrorBody {
    code: &'static str,
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, code) = match self {
            Self::InvalidRequest => (StatusCode::BAD_REQUEST, "invalid_request"),
            Self::Unauthorized => (StatusCode::UNAUTHORIZED, "unauthorized"),
            Self::NotFound => (StatusCode::NOT_FOUND, "not_found"),
            Self::Conflict => (StatusCode::CONFLICT, "state_conflict"),
            Self::Capacity => (StatusCode::TOO_MANY_REQUESTS, "capacity_exhausted"),
            Self::Unavailable => (StatusCode::SERVICE_UNAVAILABLE, "service_draining"),
            Self::Internal => (StatusCode::INTERNAL_SERVER_ERROR, "service_unavailable"),
        };
        (status, Json(ErrorBody { code })).into_response()
    }
}

#[cfg(test)]
mod tests {
    use std::{sync::Arc, time::Duration};

    use agent_core::{ModelResponse, RunBudget};
    use agent_harness::{
        AuditFailurePolicy, ExecutionHarness, HarnessConfig, M0ReadOnlyPolicy, RecoveredRun,
        RecoveredWaitingRun, RecoveryContract, RunContext, RunKey, ToolRegistry,
        testing::{FakeModelPort, InMemoryAuditSink},
    };
    use agent_persistence_sqlite::SqliteRunPersistence;
    use agent_service::{ConfiguredWorkflow, RunInput, ServiceFuture, WorkflowError, WorkflowId};
    use axum::{body::Body, http::Request};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    use super::*;

    struct CompleteWorkflow {
        id: WorkflowId,
    }

    impl ConfiguredWorkflow for CompleteWorkflow {
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
                RunBudget::new(1, 1, 1, Duration::from_secs(5)).expect("budget"),
            )
        }
        fn run(
            self: Arc<Self>,
            harness: Arc<ExecutionHarness>,
            mut context: RunContext,
            _input: RunInput,
        ) -> ServiceFuture<'static, Result<agent_service::WorkflowCompletion, WorkflowError>>
        {
            Box::pin(async move {
                harness
                    .complete_run(&mut context)
                    .await
                    .map(|()| agent_service::WorkflowCompletion::NoApplicationResult)
                    .map_err(|_| WorkflowError::Failed)
            })
        }
        fn resume_waiting(
            self: Arc<Self>,
            _harness: Arc<ExecutionHarness>,
            _recovered: RecoveredWaitingRun,
            _wait_id: DurableApprovalWaitId,
        ) -> ServiceFuture<'static, Result<agent_service::WorkflowCompletion, WorkflowError>>
        {
            Box::pin(async { Err(WorkflowError::NotRestartable) })
        }
        fn resume_recovered(
            self: Arc<Self>,
            _harness: Arc<ExecutionHarness>,
            _recovered: RecoveredRun,
        ) -> ServiceFuture<'static, Result<agent_service::WorkflowCompletion, WorkflowError>>
        {
            Box::pin(async { Err(WorkflowError::NotRestartable) })
        }
    }

    async fn service() -> (tempfile::TempDir, Arc<AgentService>) {
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
        let workflow: Arc<dyn ConfiguredWorkflow> = Arc::new(CompleteWorkflow {
            id: WorkflowId::new("test").expect("id"),
        });
        let service = Arc::new(AgentService::new(harness, store, vec![workflow]).expect("service"));
        (directory, service)
    }

    #[tokio::test]
    async fn loopback_rejects_missing_or_wrong_auth_and_origin() {
        let (_directory, service) = service().await;
        let security = HttpSecurity::loopback(
            "01234567890123456789012345678901",
            vec!["127.0.0.1:8080".to_owned()],
            vec!["http://127.0.0.1:8080".to_owned()],
        )
        .expect("security");
        let app = router(service, security);
        let missing = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/sessions")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(missing.status(), StatusCode::UNAUTHORIZED);
        let wrong_origin = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/sessions")
                    .header("authorization", "Bearer 01234567890123456789012345678901")
                    .header("host", "127.0.0.1:8080")
                    .header("origin", "http://evil.invalid")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(wrong_origin.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn command_concurrency_is_bounded_without_waiting() {
        let (_directory, service) = service().await;
        let state = HttpState {
            service,
            security: HttpSecurity::unix_socket(),
            streams: StreamLimits::new(),
            commands: Arc::new(Semaphore::new(MAX_COMMAND_QUEUE)),
            operations: OperationsState::new(
                ReadinessCache::new(ReadinessSnapshotV1 {
                    version: 1,
                    overall: ReadinessStatusV1::Unavailable,
                    dependencies: vec![],
                    workflows: vec![],
                }),
                OperationalMetrics::new(),
                BuildInfoV1 {
                    application: "test".to_owned(),
                    version: "test".to_owned(),
                    git_identity: None,
                    deployment_fingerprint: "test".to_owned(),
                    config_schema_version: 1,
                    store_schema_version: agent_harness::CURRENT_STORE_SCHEMA_VERSION,
                    event_schema_version: agent_core::CURRENT_EVENT_SCHEMA_VERSION.get(),
                    checkpoint_schema_version: agent_harness::CURRENT_CHECKPOINT_SCHEMA_VERSION,
                },
            ),
        };
        let permits = (0..MAX_COMMAND_QUEUE)
            .map(|_| state.acquire_command().expect("command permit"))
            .collect::<Vec<_>>();
        assert!(matches!(state.acquire_command(), Err(ApiError::Capacity)));
        drop(permits);
        let _released = state.acquire_command().expect("released permit");
    }

    #[tokio::test]
    async fn unix_transport_accepts_commands_and_rejects_oversized_json() {
        let (_directory, service) = service().await;
        let session = service.create_session().await.expect("session");
        let app = router(service, HttpSecurity::unix_socket());
        let body = format!(
            r#"{{"start_request_id":"{}","workflow_id":"test","input":"{}"}}"#,
            Uuid::new_v4(),
            "x".repeat(MAX_REQUEST_BODY_BYTES)
        );
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/v1/sessions/{session}/runs"))
                    .header("content-type", "application/json")
                    .body(Body::from(body))
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    }

    #[tokio::test]
    async fn operational_endpoints_are_metadata_only_and_cached() {
        let (_directory, service) = service().await;
        let app = router(service, HttpSecurity::unix_socket());
        for path in [
            "/healthz",
            "/v1/operations/readiness",
            "/v1/operations/metrics",
            "/v1/operations/version",
            "/v1/operations/runs?limit=64",
            "/v1/operations/reconciliation?limit=64",
        ] {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .uri(path)
                        .body(Body::empty())
                        .expect("request"),
                )
                .await
                .expect("response");
            assert_eq!(response.status(), StatusCode::OK, "{path}");
            let body = response
                .into_body()
                .collect()
                .await
                .expect("body")
                .to_bytes();
            let text = std::str::from_utf8(&body).expect("UTF-8 JSON");
            for forbidden in [
                "prompt",
                "final_answer",
                "evidence",
                "credential",
                "ciphertext",
                "capsule",
                "action_content",
                "tool_result",
            ] {
                assert!(!text.contains(forbidden), "{path} leaked {forbidden}");
            }
        }
    }

    #[tokio::test]
    async fn sse_reconnects_from_sequence_and_bounds_slow_connections() {
        let (_directory, service) = service().await;
        let session = service.create_session().await.expect("session");
        let workflow = WorkflowId::new("test").expect("workflow");
        let run = service
            .start_run(
                session,
                Uuid::new_v4(),
                &workflow,
                RunInput::new(b"input".to_vec()).expect("input"),
            )
            .await
            .expect("start");
        tokio::time::sleep(Duration::from_millis(20)).await;
        let app = router(service.clone(), HttpSecurity::unix_socket());
        let uri = format!(
            "/v1/sessions/{session}/runs/{}/events/stream?after=0",
            run.run_id
        );
        let mut streams = Vec::new();
        for _ in 0..MAX_SSE_CONNECTIONS_PER_RUN {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .uri(&uri)
                        .body(Body::empty())
                        .expect("request"),
                )
                .await
                .expect("response");
            assert_eq!(response.status(), StatusCode::OK);
            streams.push(response);
        }
        let saturated = app
            .oneshot(
                Request::builder()
                    .uri(&uri)
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(saturated.status(), StatusCode::TOO_MANY_REQUESTS);

        let first = streams.remove(0);
        let frame = tokio::time::timeout(Duration::from_secs(1), first.into_body().frame())
            .await
            .expect("SSE frame deadline")
            .expect("SSE frame")
            .expect("valid frame");
        let data = frame.into_data().expect("data frame");
        let text = std::str::from_utf8(&data).expect("UTF-8 SSE");
        assert!(text.contains("id: 1"));
        drop(streams);
        assert_eq!(
            service
                .get_run_status(RunKey::new(run.run_id, session))
                .await
                .expect("status")
                .disposition,
            agent_service::RunDispositionV1::Completed
        );
    }
}
