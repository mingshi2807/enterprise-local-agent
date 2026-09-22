use std::{
    env, fmt,
    net::IpAddr,
    path::{Component, Path, PathBuf},
    time::Duration,
};

use reqwest::{Client, Method, StatusCode, header};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use url::{Host, Url};
use zeroize::Zeroize;

const MAX_RESPONSE_BYTES: usize = 64 * 1024;
const MAX_ITEMS: usize = 64;
const MAX_TEXT_BYTES: usize = 256;
const MAX_RUN_INPUT_BYTES: usize = 8 * 1024;
const MAX_FINAL_ANSWER_BYTES: usize = 8 * 1024;
const MAX_CITATION_FIELD_BYTES: usize = 1_024;
const MAX_RESULT_CITATIONS: usize = 8;
const MAX_KNOWLEDGE_BACKENDS: usize = 8;
const READONLY_WORKFLOW_ID: &str = "enterprise-engineering-readonly-v1";
const LOCALWRITE_WORKFLOW_ID: &str = "enterprise-engineering-localwrite-v1";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ServiceLifecycleV1 {
    Serving,
    Draining,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReadinessStatusV1 {
    Ready,
    Degraded,
    Unavailable,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HealthV1 {
    version: u16,
    lifecycle: ServiceLifecycleV1,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DependencyReadinessV1 {
    dependency: String,
    status: ReadinessStatusV1,
    code: String,
    checked_unix_seconds: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowReadinessV1 {
    workflow: String,
    enabled: bool,
    required: bool,
    status: ReadinessStatusV1,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReadinessSnapshotV1 {
    version: u16,
    overall: ReadinessStatusV1,
    dependencies: Vec<DependencyReadinessV1>,
    workflows: Vec<WorkflowReadinessV1>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BuildInfoV1 {
    application: String,
    version: String,
    git_identity: Option<String>,
    deployment_fingerprint: String,
    config_schema_version: u16,
    store_schema_version: u16,
    event_schema_version: u16,
    checkpoint_schema_version: u16,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionV1 {
    session_id: String,
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
pub struct ApplicationCitationV1 {
    evidence_id: String,
    backend: String,
    source_id: String,
    reference_id: String,
    provenance: Option<String>,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind", deny_unknown_fields)]
pub enum ApplicationResultV1 {
    FinalAnswer {
        answer: String,
        citations: Vec<ApplicationCitationV1>,
    },
    LocalWriteCompleted {
        tool_call_id: String,
    },
    ApprovalDenied,
}

impl fmt::Debug for ApplicationResultV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::FinalAnswer { answer, citations } => formatter
                .debug_struct("FinalAnswer")
                .field("answer", &"[REDACTED]")
                .field("answer_bytes", &answer.len())
                .field("citation_count", &citations.len())
                .finish(),
            Self::LocalWriteCompleted { tool_call_id } => formatter
                .debug_struct("LocalWriteCompleted")
                .field("tool_call_id", tool_call_id)
                .finish(),
            Self::ApprovalDenied => formatter.write_str("ApprovalDenied"),
        }
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunViewV1 {
    session_id: String,
    run_id: String,
    disposition: RunDispositionV1,
    last_sequence: Option<u64>,
    outcome: Option<RunOutcomeV1>,
    workflow_id: Option<String>,
    result: Option<ApplicationResultV1>,
    duration_millis: Option<u64>,
}

impl fmt::Debug for RunViewV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RunViewV1")
            .field("session_id", &self.session_id)
            .field("run_id", &self.run_id)
            .field("disposition", &self.disposition)
            .field("last_sequence", &self.last_sequence)
            .field("outcome", &self.outcome)
            .field("workflow_id", &self.workflow_id)
            .field("result", &self.result.as_ref().map(|_| "[REDACTED]"))
            .field("duration_millis", &self.duration_millis)
            .finish()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ServiceEventCategoryV2 {
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ServiceEventPhaseV2 {
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

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServiceEventV2 {
    version: u16,
    sequence: u64,
    run_id: String,
    category: ServiceEventCategoryV2,
    phase: ServiceEventPhaseV2,
    correlation_id: Option<String>,
    workflow_id: Option<String>,
    knowledge_backends: Vec<String>,
    graph_node_id: Option<String>,
    budget_usage: Option<u32>,
    budget_limit: Option<u32>,
}

#[derive(Serialize)]
#[serde(deny_unknown_fields)]
struct StartRunBody<'a> {
    start_request_id: &'a str,
    workflow_id: &'a str,
    input: &'a str,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WaitingStateV1 {
    Waiting,
    Approved,
    Denied,
    Executing,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WaitingApprovalV1 {
    session_id: String,
    run_id: String,
    wait_id: String,
    row_version: u64,
    state: WaitingStateV1,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WaitingPageV1 {
    items: Vec<WaitingApprovalV1>,
    next_run_id: Option<String>,
    next_wait_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ServiceApprovalPreviewV1 {
    wait_id: String,
    row_version: u64,
    summary: String,
    target: String,
}

#[derive(Clone, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DesktopApprovalPreviewV1 {
    wait_id: String,
    row_version: u64,
    operation: &'static str,
    target: String,
    content_bytes: u16,
}

impl fmt::Debug for DesktopApprovalPreviewV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DesktopApprovalPreviewV1")
            .field("wait_id", &self.wait_id)
            .field("row_version", &self.row_version)
            .field("operation", &self.operation)
            .field("target", &self.target)
            .field("content_bytes", &self.content_bytes)
            .finish()
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalDecisionV1 {
    Approve,
    Deny,
}

#[derive(Serialize)]
#[serde(deny_unknown_fields)]
struct DecisionBody {
    expected_row_version: u64,
    decision: ApprovalDecisionV1,
}

#[derive(Serialize)]
#[serde(deny_unknown_fields)]
struct ResumeBody<'a> {
    wait_id: &'a str,
}

#[derive(Serialize)]
#[serde(deny_unknown_fields)]
struct AbortBody {
    expected_row_version: u64,
}

trait BoundedResponse {
    fn validate(&self) -> Result<(), LocalServiceError>;
}

impl BoundedResponse for HealthV1 {
    fn validate(&self) -> Result<(), LocalServiceError> {
        (self.version == 1)
            .then_some(())
            .ok_or(LocalServiceError::InvalidResponse)
    }
}

impl BoundedResponse for ReadinessSnapshotV1 {
    fn validate(&self) -> Result<(), LocalServiceError> {
        if self.version != 1
            || self.dependencies.len() > MAX_ITEMS
            || self.workflows.len() > MAX_ITEMS
            || self
                .dependencies
                .iter()
                .any(|item| !bounded_text(&item.dependency) || !bounded_text(&item.code))
            || self
                .workflows
                .iter()
                .any(|item| !bounded_text(&item.workflow))
        {
            return Err(LocalServiceError::InvalidResponse);
        }
        Ok(())
    }
}

impl BoundedResponse for BuildInfoV1 {
    fn validate(&self) -> Result<(), LocalServiceError> {
        let valid = [
            self.application.as_str(),
            self.version.as_str(),
            self.deployment_fingerprint.as_str(),
        ]
        .into_iter()
        .all(bounded_text)
            && self.git_identity.as_deref().is_none_or(bounded_text);
        valid
            .then_some(())
            .ok_or(LocalServiceError::InvalidResponse)
    }
}

impl BoundedResponse for SessionV1 {
    fn validate(&self) -> Result<(), LocalServiceError> {
        valid_uuid(&self.session_id)
            .then_some(())
            .ok_or(LocalServiceError::InvalidResponse)
    }
}

impl BoundedResponse for RunViewV1 {
    fn validate(&self) -> Result<(), LocalServiceError> {
        if !valid_uuid(&self.session_id)
            || !valid_uuid(&self.run_id)
            || self.workflow_id.as_deref().is_some_and(|workflow| {
                workflow != READONLY_WORKFLOW_ID && workflow != LOCALWRITE_WORKFLOW_ID
            })
            || self
                .result
                .as_ref()
                .is_some_and(|result| !valid_application_result(result))
        {
            return Err(LocalServiceError::InvalidResponse);
        }
        Ok(())
    }
}

impl BoundedResponse for Vec<ServiceEventV2> {
    fn validate(&self) -> Result<(), LocalServiceError> {
        if self.len() > MAX_ITEMS
            || self.iter().any(|event| {
                event.version != 2
                    || !valid_uuid(&event.run_id)
                    || event
                        .correlation_id
                        .as_deref()
                        .is_some_and(|value| !bounded_text(value))
                    || event
                        .workflow_id
                        .as_deref()
                        .is_some_and(|value| {
                            value != READONLY_WORKFLOW_ID && value != LOCALWRITE_WORKFLOW_ID
                        })
                    || event.knowledge_backends.len() > MAX_KNOWLEDGE_BACKENDS
                    || event
                        .knowledge_backends
                        .iter()
                        .any(|value| !bounded_text(value))
                    || event
                        .graph_node_id
                        .as_deref()
                        .is_some_and(|value| !bounded_text(value))
                    || matches!((event.budget_usage, event.budget_limit), (Some(used), Some(limit)) if used > limit)
                    || event.budget_usage.is_some() != event.budget_limit.is_some()
            })
        {
            return Err(LocalServiceError::InvalidResponse);
        }
        Ok(())
    }
}

impl BoundedResponse for WaitingPageV1 {
    fn validate(&self) -> Result<(), LocalServiceError> {
        if self.items.len() > MAX_ITEMS
            || self.items.iter().any(|item| {
                !valid_uuid(&item.session_id)
                    || !valid_uuid(&item.run_id)
                    || !valid_uuid(&item.wait_id)
            })
            || self
                .next_run_id
                .as_deref()
                .is_some_and(|id| !valid_uuid(id))
            || self
                .next_wait_id
                .as_deref()
                .is_some_and(|id| !valid_uuid(id))
            || self.next_run_id.is_some() != self.next_wait_id.is_some()
        {
            return Err(LocalServiceError::InvalidResponse);
        }
        Ok(())
    }
}

impl BoundedResponse for ServiceApprovalPreviewV1 {
    fn validate(&self) -> Result<(), LocalServiceError> {
        if !valid_uuid(&self.wait_id)
            || !bounded_text(&self.summary)
            || !safe_relative_target(&self.target)
        {
            return Err(LocalServiceError::InvalidResponse);
        }
        Ok(())
    }
}

fn valid_application_result(result: &ApplicationResultV1) -> bool {
    match result {
        ApplicationResultV1::FinalAnswer { answer, citations } => {
            !answer.is_empty()
                && answer.len() <= MAX_FINAL_ANSWER_BYTES
                && citations.len() <= MAX_RESULT_CITATIONS
                && citations.iter().all(|citation| {
                    [
                        citation.evidence_id.as_str(),
                        citation.backend.as_str(),
                        citation.source_id.as_str(),
                        citation.reference_id.as_str(),
                    ]
                    .into_iter()
                    .chain(citation.provenance.as_deref())
                    .all(bounded_citation_text)
                })
        }
        ApplicationResultV1::LocalWriteCompleted { tool_call_id } => valid_uuid(tool_call_id),
        ApplicationResultV1::ApprovalDenied => true,
    }
}

fn bounded_citation_text(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_CITATION_FIELD_BYTES
        && value.chars().all(|character| !character.is_control())
}

fn valid_uuid(value: &str) -> bool {
    uuid::Uuid::parse_str(value).is_ok()
}

fn bounded_text(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_TEXT_BYTES
        && value.chars().all(|character| !character.is_control())
}

fn safe_relative_target(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 1_024
        && !value.chars().any(char::is_control)
        && !Path::new(value).is_absolute()
        && Path::new(value)
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
}

struct BearerSecret(String);

impl BearerSecret {
    fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for BearerSecret {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("BearerSecret([REDACTED])")
    }
}

impl Drop for BearerSecret {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

enum LocalServiceTransport {
    #[cfg(unix)]
    UnixSocket { path: PathBuf },
    Loopback {
        base_url: Url,
        bearer: BearerSecret,
        origin: header::HeaderValue,
    },
}

impl fmt::Debug for LocalServiceTransport {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            #[cfg(unix)]
            Self::UnixSocket { path } => formatter
                .debug_struct("UnixSocket")
                .field("path", path)
                .finish(),
            Self::Loopback { base_url, .. } => formatter
                .debug_struct("Loopback")
                .field("base_url", base_url)
                .field("bearer", &"[REDACTED]")
                .finish(),
        }
    }
}

pub struct LocalServiceClient {
    client: Client,
    transport: LocalServiceTransport,
}

impl fmt::Debug for LocalServiceClient {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LocalServiceClient")
            .field("transport", &self.transport)
            .finish_non_exhaustive()
    }
}

impl LocalServiceClient {
    pub fn from_environment() -> Result<Self, LocalServiceError> {
        let socket = env::var_os("ELA_DESKTOP_SERVICE_SOCKET");
        let base_url = env::var("ELA_DESKTOP_SERVICE_URL").ok();
        if socket.is_some() && base_url.is_some() {
            return Err(LocalServiceError::InvalidConfiguration);
        }

        #[cfg(unix)]
        if let Some(path) = socket {
            return Self::unix_socket(PathBuf::from(path));
        }

        if let Some(base_url) = base_url {
            let bearer = env::var("ELA_DESKTOP_SERVICE_BEARER")
                .map_err(|_| LocalServiceError::InvalidConfiguration)?;
            let origin = env::var("ELA_DESKTOP_SERVICE_ORIGIN")
                .map_err(|_| LocalServiceError::InvalidConfiguration)?;
            return Self::loopback(&base_url, bearer, &origin);
        }

        #[cfg(unix)]
        {
            let runtime_directory = env::var_os("XDG_RUNTIME_DIR")
                .map(PathBuf::from)
                .ok_or(LocalServiceError::InvalidConfiguration)?;
            Self::unix_socket(runtime_directory.join("enterprise-local-agent.sock"))
        }

        #[cfg(not(unix))]
        Err(LocalServiceError::InvalidConfiguration)
    }

    #[cfg(unix)]
    fn unix_socket(path: PathBuf) -> Result<Self, LocalServiceError> {
        if !path.is_absolute() || path.as_os_str().len() > 4_096 {
            return Err(LocalServiceError::InvalidConfiguration);
        }
        let client = Client::builder()
            .unix_socket(path.clone())
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(REQUEST_TIMEOUT)
            .build()
            .map_err(|_| LocalServiceError::InvalidConfiguration)?;
        Ok(Self {
            client,
            transport: LocalServiceTransport::UnixSocket { path },
        })
    }

    fn loopback(base_url: &str, bearer: String, origin: &str) -> Result<Self, LocalServiceError> {
        let base_url = Url::parse(base_url).map_err(|_| LocalServiceError::InvalidConfiguration)?;
        let host_is_loopback = match base_url.host() {
            Some(Host::Ipv4(address)) => IpAddr::V4(address).is_loopback(),
            Some(Host::Ipv6(address)) => IpAddr::V6(address).is_loopback(),
            _ => false,
        };
        if base_url.scheme() != "http"
            || !host_is_loopback
            || !base_url.username().is_empty()
            || base_url.password().is_some()
            || base_url.query().is_some()
            || base_url.fragment().is_some()
            || base_url.path() != "/"
            || bearer.len() < 32
            || bearer.len() > 4_096
        {
            return Err(LocalServiceError::InvalidConfiguration);
        }
        let origin = header::HeaderValue::from_str(origin)
            .map_err(|_| LocalServiceError::InvalidConfiguration)?;
        if origin.as_bytes().len() > MAX_TEXT_BYTES {
            return Err(LocalServiceError::InvalidConfiguration);
        }
        let client = Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(REQUEST_TIMEOUT)
            .build()
            .map_err(|_| LocalServiceError::InvalidConfiguration)?;
        Ok(Self {
            client,
            transport: LocalServiceTransport::Loopback {
                base_url,
                bearer: BearerSecret(bearer),
                origin,
            },
        })
    }

    pub async fn health(&self) -> Result<HealthV1, LocalServiceError> {
        self.get("healthz").await
    }

    pub async fn readiness(&self) -> Result<ReadinessSnapshotV1, LocalServiceError> {
        self.get("v1/operations/readiness").await
    }

    pub async fn version(&self) -> Result<BuildInfoV1, LocalServiceError> {
        self.get("v1/operations/version").await
    }

    pub async fn create_session(&self) -> Result<SessionV1, LocalServiceError> {
        let response = self
            .request(Method::POST, "v1/sessions")?
            .header(header::ACCEPT, "application/json")
            .send()
            .await
            .map_err(|_| LocalServiceError::Unavailable)?;
        self.decode_json(response, StatusCode::OK).await
    }

    pub async fn start_readonly_run(
        &self,
        session_id: &str,
        start_request_id: &str,
        input: &str,
    ) -> Result<RunViewV1, LocalServiceError> {
        self.start_run(session_id, start_request_id, input, READONLY_WORKFLOW_ID)
            .await
    }

    pub async fn start_localwrite_run(
        &self,
        session_id: &str,
        start_request_id: &str,
        input: &str,
    ) -> Result<RunViewV1, LocalServiceError> {
        self.start_run(session_id, start_request_id, input, LOCALWRITE_WORKFLOW_ID)
            .await
    }

    async fn start_run(
        &self,
        session_id: &str,
        start_request_id: &str,
        input: &str,
        workflow_id: &'static str,
    ) -> Result<RunViewV1, LocalServiceError> {
        if !valid_uuid(session_id)
            || !valid_uuid(start_request_id)
            || input.is_empty()
            || input.len() > MAX_RUN_INPUT_BYTES
        {
            return Err(LocalServiceError::InvalidRequest);
        }
        let body = StartRunBody {
            start_request_id,
            workflow_id,
            input,
        };
        let body = serde_json::to_vec(&body).map_err(|_| LocalServiceError::InvalidRequest)?;
        let response = self
            .request(Method::POST, &format!("v1/sessions/{session_id}/runs"))?
            .header(header::ACCEPT, "application/json")
            .header(header::CONTENT_TYPE, "application/json")
            .body(body)
            .send()
            .await
            .map_err(|_| LocalServiceError::Unavailable)?;
        let run: RunViewV1 = self.decode_json(response, StatusCode::OK).await?;
        if run.session_id != session_id || run.workflow_id.as_deref() != Some(workflow_id) {
            return Err(LocalServiceError::InvalidResponse);
        }
        Ok(run)
    }

    pub async fn run_status(
        &self,
        session_id: &str,
        run_id: &str,
    ) -> Result<RunViewV1, LocalServiceError> {
        validate_run_key(session_id, run_id)?;
        let run: RunViewV1 = self
            .get(&format!("v1/sessions/{session_id}/runs/{run_id}"))
            .await?;
        if run.session_id != session_id || run.run_id != run_id {
            return Err(LocalServiceError::InvalidResponse);
        }
        Ok(run)
    }

    pub async fn cancel_run(
        &self,
        session_id: &str,
        run_id: &str,
    ) -> Result<(), LocalServiceError> {
        validate_run_key(session_id, run_id)?;
        let response = self
            .request(
                Method::POST,
                &format!("v1/sessions/{session_id}/runs/{run_id}/cancel"),
            )?
            .send()
            .await
            .map_err(|_| LocalServiceError::Unavailable)?;
        self.require_status(response.status(), StatusCode::ACCEPTED)
    }

    pub async fn read_events(
        &self,
        session_id: &str,
        run_id: &str,
        after: Option<u64>,
    ) -> Result<Vec<ServiceEventV2>, LocalServiceError> {
        validate_run_key(session_id, run_id)?;
        let path = after.map_or_else(
            || format!("v1/sessions/{session_id}/runs/{run_id}/events?limit={MAX_ITEMS}"),
            |cursor| {
                format!(
                    "v1/sessions/{session_id}/runs/{run_id}/events?after={cursor}&limit={MAX_ITEMS}"
                )
            },
        );
        let events: Vec<ServiceEventV2> = self.get(&path).await?;
        validate_event_page(&events, run_id, after)?;
        Ok(events)
    }

    pub async fn list_waiting(&self) -> Result<WaitingPageV1, LocalServiceError> {
        self.get(&format!("v1/approvals?limit={MAX_ITEMS}")).await
    }

    pub async fn approval_preview(
        &self,
        session_id: &str,
        run_id: &str,
        wait_id: &str,
    ) -> Result<DesktopApprovalPreviewV1, LocalServiceError> {
        validate_wait_key(session_id, run_id, wait_id)?;
        let preview: ServiceApprovalPreviewV1 = self
            .get(&format!(
                "v1/sessions/{session_id}/runs/{run_id}/approvals/{wait_id}/preview"
            ))
            .await?;
        if preview.wait_id != wait_id {
            return Err(LocalServiceError::InvalidResponse);
        }
        normalize_preview(preview)
    }

    pub async fn submit_decision(
        &self,
        session_id: &str,
        run_id: &str,
        wait_id: &str,
        expected_row_version: u64,
        decision: ApprovalDecisionV1,
    ) -> Result<(), LocalServiceError> {
        validate_wait_key(session_id, run_id, wait_id)?;
        self.post_json_status(
            &format!("v1/sessions/{session_id}/runs/{run_id}/approvals/{wait_id}/decision"),
            &DecisionBody {
                expected_row_version,
                decision,
            },
            StatusCode::NO_CONTENT,
        )
        .await
    }

    pub async fn resume_waiting(
        &self,
        session_id: &str,
        run_id: &str,
        wait_id: &str,
    ) -> Result<(), LocalServiceError> {
        validate_wait_key(session_id, run_id, wait_id)?;
        self.post_json_status(
            &format!("v1/sessions/{session_id}/runs/{run_id}/resume"),
            &ResumeBody { wait_id },
            StatusCode::ACCEPTED,
        )
        .await
    }

    pub async fn abort_waiting(
        &self,
        session_id: &str,
        run_id: &str,
        wait_id: &str,
        expected_row_version: u64,
    ) -> Result<(), LocalServiceError> {
        validate_wait_key(session_id, run_id, wait_id)?;
        self.post_json_status(
            &format!("v1/sessions/{session_id}/runs/{run_id}/approvals/{wait_id}/abort"),
            &AbortBody {
                expected_row_version,
            },
            StatusCode::NO_CONTENT,
        )
        .await
    }

    async fn post_json_status<T: Serialize>(
        &self,
        path: &str,
        body: &T,
        expected: StatusCode,
    ) -> Result<(), LocalServiceError> {
        let body = serde_json::to_vec(body).map_err(|_| LocalServiceError::InvalidRequest)?;
        let response = self
            .request(Method::POST, path)?
            .header(header::ACCEPT, "application/json")
            .header(header::CONTENT_TYPE, "application/json")
            .body(body)
            .send()
            .await
            .map_err(|_| LocalServiceError::Unavailable)?;
        self.require_status(response.status(), expected)
    }

    async fn get<T>(&self, path: &str) -> Result<T, LocalServiceError>
    where
        T: DeserializeOwned + BoundedResponse,
    {
        let response = self
            .request(Method::GET, path)?
            .header(header::ACCEPT, "application/json")
            .send()
            .await
            .map_err(|_| LocalServiceError::Unavailable)?;
        self.decode_json(response, StatusCode::OK).await
    }

    fn request(
        &self,
        method: Method,
        path: &str,
    ) -> Result<reqwest::RequestBuilder, LocalServiceError> {
        if path.is_empty() || path.starts_with('/') || path.len() > 1_024 {
            return Err(LocalServiceError::InvalidConfiguration);
        }
        Ok(match &self.transport {
            #[cfg(unix)]
            LocalServiceTransport::UnixSocket { .. } => self
                .client
                .request(method, format!("http://localhost/{path}")),
            LocalServiceTransport::Loopback {
                base_url,
                bearer,
                origin,
            } => {
                let url = base_url
                    .join(path)
                    .map_err(|_| LocalServiceError::InvalidConfiguration)?;
                self.client
                    .request(method, url)
                    .bearer_auth(bearer.expose())
                    .header(header::ORIGIN, origin.clone())
            }
        })
    }

    fn require_status(
        &self,
        actual: StatusCode,
        expected: StatusCode,
    ) -> Result<(), LocalServiceError> {
        if actual == expected {
            return Ok(());
        }
        Err(error_for_status(actual))
    }

    async fn decode_json<T>(
        &self,
        mut response: reqwest::Response,
        expected: StatusCode,
    ) -> Result<T, LocalServiceError>
    where
        T: DeserializeOwned + BoundedResponse,
    {
        self.require_status(response.status(), expected)?;
        if response
            .content_length()
            .is_some_and(|length| length > MAX_RESPONSE_BYTES as u64)
        {
            return Err(LocalServiceError::ResponseTooLarge);
        }
        let mut body = Vec::with_capacity(1_024);
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| LocalServiceError::Unavailable)?
        {
            if body.len().saturating_add(chunk.len()) > MAX_RESPONSE_BYTES {
                return Err(LocalServiceError::ResponseTooLarge);
            }
            body.extend_from_slice(&chunk);
        }
        let value: T =
            serde_json::from_slice(&body).map_err(|_| LocalServiceError::InvalidResponse)?;
        value.validate()?;
        Ok(value)
    }
}

fn validate_run_key(session_id: &str, run_id: &str) -> Result<(), LocalServiceError> {
    if !valid_uuid(session_id) || !valid_uuid(run_id) {
        return Err(LocalServiceError::InvalidRequest);
    }
    Ok(())
}

fn validate_wait_key(
    session_id: &str,
    run_id: &str,
    wait_id: &str,
) -> Result<(), LocalServiceError> {
    validate_run_key(session_id, run_id)?;
    if !valid_uuid(wait_id) {
        return Err(LocalServiceError::InvalidRequest);
    }
    Ok(())
}

fn normalize_preview(
    preview: ServiceApprovalPreviewV1,
) -> Result<DesktopApprovalPreviewV1, LocalServiceError> {
    let bytes = preview
        .summary
        .strip_prefix("workspace_write_file (")
        .and_then(|value| value.strip_suffix(" bytes)"))
        .and_then(|value| value.parse::<u16>().ok())
        .filter(|value| *value <= 4 * 1024)
        .ok_or(LocalServiceError::InvalidResponse)?;
    Ok(DesktopApprovalPreviewV1 {
        wait_id: preview.wait_id,
        row_version: preview.row_version,
        operation: "Write workspace file",
        target: preview.target,
        content_bytes: bytes,
    })
}

fn validate_event_page(
    events: &[ServiceEventV2],
    run_id: &str,
    cursor: Option<u64>,
) -> Result<(), LocalServiceError> {
    let mut expected = cursor.map_or(0, |value| value.saturating_add(1));
    for event in events {
        if event.run_id != run_id || event.sequence != expected {
            return Err(LocalServiceError::InvalidResponse);
        }
        expected = expected.saturating_add(1);
    }
    Ok(())
}

fn error_for_status(status: StatusCode) -> LocalServiceError {
    match status {
        StatusCode::BAD_REQUEST => LocalServiceError::InvalidRequest,
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => LocalServiceError::Unauthorized,
        StatusCode::NOT_FOUND => LocalServiceError::NotFound,
        StatusCode::CONFLICT => LocalServiceError::Conflict,
        StatusCode::TOO_MANY_REQUESTS => LocalServiceError::Capacity,
        StatusCode::SERVICE_UNAVAILABLE => LocalServiceError::Draining,
        _ => LocalServiceError::Unavailable,
    }
}

#[derive(Clone, Copy, Debug, thiserror::Error)]
pub enum LocalServiceError {
    #[error("invalid desktop service configuration")]
    InvalidConfiguration,
    #[error("local service unavailable")]
    Unavailable,
    #[error("local service response exceeded its bound")]
    ResponseTooLarge,
    #[error("invalid local service response")]
    InvalidResponse,
    #[error("invalid desktop conversation request")]
    InvalidRequest,
    #[error("desktop principal is not authorized")]
    Unauthorized,
    #[error("conversation resource was not found")]
    NotFound,
    #[error("conversation state conflict")]
    Conflict,
    #[error("local service capacity exhausted")]
    Capacity,
    #[error("local service is draining")]
    Draining,
}

impl LocalServiceError {
    pub const fn code(self) -> &'static str {
        match self {
            Self::InvalidConfiguration => "invalid_service_configuration",
            Self::Unavailable => "service_unavailable",
            Self::ResponseTooLarge => "service_response_too_large",
            Self::InvalidResponse => "invalid_service_response",
            Self::InvalidRequest => "invalid_request",
            Self::Unauthorized => "unauthorized",
            Self::NotFound => "not_found",
            Self::Conflict => "state_conflict",
            Self::Capacity => "capacity_exhausted",
            Self::Draining => "service_draining",
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use super::*;

    const HEALTH: &str = r#"{"version":1,"lifecycle":"serving"}"#;

    #[tokio::test]
    async fn loopback_request_keeps_bearer_in_rust_and_sets_required_headers() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .unwrap_or_else(|error| panic!("bind test listener: {error}"));
        let address = listener
            .local_addr()
            .unwrap_or_else(|error| panic!("read listener address: {error}"));
        let captured = Arc::new(tokio::sync::Mutex::new(String::new()));
        let server_capture = captured.clone();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener
                .accept()
                .await
                .unwrap_or_else(|error| panic!("accept request: {error}"));
            let mut request = vec![0_u8; 4_096];
            let read = socket
                .read(&mut request)
                .await
                .unwrap_or_else(|error| panic!("read request: {error}"));
            *server_capture.lock().await = String::from_utf8_lossy(&request[..read]).into();
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{HEALTH}",
                HEALTH.len()
            );
            socket
                .write_all(response.as_bytes())
                .await
                .unwrap_or_else(|error| panic!("write response: {error}"));
        });
        let secret = "a".repeat(32);
        let client = LocalServiceClient::loopback(
            &format!("http://{address}/"),
            secret.clone(),
            "tauri://localhost",
        )
        .unwrap_or_else(|error| panic!("create client: {error}"));

        let response = client
            .health()
            .await
            .unwrap_or_else(|error| panic!("read health: {error}"));
        server
            .await
            .unwrap_or_else(|error| panic!("join server: {error}"));

        assert_eq!(response.lifecycle, ServiceLifecycleV1::Serving);
        let request = captured.lock().await;
        assert!(request.contains("GET /healthz HTTP/1.1"));
        assert!(request.contains(&format!("authorization: Bearer {secret}")));
        assert!(request.contains("origin: tauri://localhost"));
        assert!(!format!("{client:?}").contains(&secret));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn unix_socket_transport_reads_health_without_credentials() {
        let directory = tempfile::tempdir()
            .unwrap_or_else(|error| panic!("create temporary directory: {error}"));
        let path = directory.path().join("service.sock");
        let listener = tokio::net::UnixListener::bind(&path)
            .unwrap_or_else(|error| panic!("bind Unix listener: {error}"));
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener
                .accept()
                .await
                .unwrap_or_else(|error| panic!("accept request: {error}"));
            let mut request = vec![0_u8; 2_048];
            let _ = socket
                .read(&mut request)
                .await
                .unwrap_or_else(|error| panic!("read request: {error}"));
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{HEALTH}",
                HEALTH.len()
            );
            socket
                .write_all(response.as_bytes())
                .await
                .unwrap_or_else(|error| panic!("write response: {error}"));
        });
        let client = LocalServiceClient::unix_socket(path)
            .unwrap_or_else(|error| panic!("create Unix client: {error}"));

        let response = client
            .health()
            .await
            .unwrap_or_else(|error| panic!("read health: {error}"));
        server
            .await
            .unwrap_or_else(|error| panic!("join server: {error}"));

        assert_eq!(response.version, 1);
    }

    #[test]
    fn loopback_rejects_non_loopback_and_short_credentials() {
        assert!(
            LocalServiceClient::loopback(
                "http://192.168.1.20:8080/",
                "a".repeat(32),
                "tauri://localhost"
            )
            .is_err()
        );
        assert!(
            LocalServiceClient::loopback(
                "http://127.0.0.1:8080/",
                "short".to_owned(),
                "tauri://localhost"
            )
            .is_err()
        );
    }

    #[test]
    fn strict_response_contract_rejects_unknown_fields_and_oversized_metadata() {
        let unknown = serde_json::from_str::<HealthV1>(
            r#"{"version":1,"lifecycle":"serving","secret":"value"}"#,
        );
        assert!(unknown.is_err());

        let response = BuildInfoV1 {
            application: "x".repeat(MAX_TEXT_BYTES + 1),
            version: "0.1.0".to_owned(),
            git_identity: None,
            deployment_fingerprint: "test".to_owned(),
            config_schema_version: 2,
            store_schema_version: 2,
            event_schema_version: 9,
            checkpoint_schema_version: 4,
        };
        assert!(response.validate().is_err());
    }

    #[test]
    fn conversation_contract_is_bounded_strict_and_redacted() {
        let result = ApplicationResultV1::FinalAnswer {
            answer: "sensitive answer".to_owned(),
            citations: vec![ApplicationCitationV1 {
                evidence_id: "evidence-1".to_owned(),
                backend: "standards".to_owned(),
                source_id: "iso".to_owned(),
                reference_id: "8.4".to_owned(),
                provenance: Some("trusted reference".to_owned()),
            }],
        };
        let run = RunViewV1 {
            session_id: "11111111-1111-4111-8111-111111111111".to_owned(),
            run_id: "22222222-2222-4222-8222-222222222222".to_owned(),
            disposition: RunDispositionV1::Completed,
            last_sequence: Some(2),
            outcome: Some(RunOutcomeV1::Completed),
            workflow_id: Some(READONLY_WORKFLOW_ID.to_owned()),
            result: Some(result),
            duration_millis: Some(42),
        };

        assert!(run.validate().is_ok());
        let debug = format!("{run:?}");
        assert!(debug.contains("[REDACTED]"));
        assert!(!debug.contains("sensitive answer"));
        assert!(
            serde_json::from_str::<RunViewV1>(
                r#"{"session_id":"11111111-1111-4111-8111-111111111111","run_id":"22222222-2222-4222-8222-222222222222","disposition":"running","last_sequence":null,"outcome":null,"workflow_id":"enterprise-engineering-readonly-v1","result":null,"duration_millis":null,"prompt":"secret"}"#,
            )
            .is_err()
        );
    }

    #[test]
    fn service_events_require_metadata_only_v2_contract() {
        let event = ServiceEventV2 {
            version: 2,
            sequence: 0,
            run_id: "22222222-2222-4222-8222-222222222222".to_owned(),
            category: ServiceEventCategoryV2::Knowledge,
            phase: ServiceEventPhaseV2::Started,
            correlation_id: Some("retrieval-1".to_owned()),
            workflow_id: Some(READONLY_WORKFLOW_ID.to_owned()),
            knowledge_backends: vec!["standards".to_owned()],
            graph_node_id: Some("retrieve".to_owned()),
            budget_usage: Some(1),
            budget_limit: Some(8),
        };
        assert!(vec![event.clone()].validate().is_ok());
        assert!(validate_event_page(std::slice::from_ref(&event), &event.run_id, None).is_ok());
        let mut gap = event;
        gap.sequence = 1;
        assert!(validate_event_page(&[gap], "22222222-2222-4222-8222-222222222222", None).is_err());
        assert!(
            serde_json::from_str::<ServiceEventV2>(
                r#"{"version":2,"sequence":1,"run_id":"22222222-2222-4222-8222-222222222222","category":"model","phase":"started","correlation_id":null,"workflow_id":"enterprise-engineering-readonly-v1","knowledge_backends":[],"graph_node_id":null,"budget_usage":null,"budget_limit":null,"raw_model_output":"secret"}"#,
            )
            .is_err()
        );
    }

    #[test]
    fn approval_preview_is_narrow_bounded_and_redacted() {
        let preview = normalize_preview(ServiceApprovalPreviewV1 {
            wait_id: "33333333-3333-4333-8333-333333333333".to_owned(),
            row_version: 2,
            summary: "workspace_write_file (42 bytes)".to_owned(),
            target: "reports/result.txt".to_owned(),
        })
        .unwrap_or_else(|error| panic!("normalize trusted preview: {error}"));

        assert_eq!(preview.operation, "Write workspace file");
        assert_eq!(preview.target, "reports/result.txt");
        assert_eq!(preview.content_bytes, 42);
        assert!(!format!("{preview:?}").contains("file contents"));
        assert!(
            normalize_preview(ServiceApprovalPreviewV1 {
                wait_id: "33333333-3333-4333-8333-333333333333".to_owned(),
                row_version: 2,
                summary: "workspace_write_file (4097 bytes)".to_owned(),
                target: "reports/result.txt".to_owned(),
            })
            .is_err()
        );
        let unsafe_target = ServiceApprovalPreviewV1 {
            wait_id: "33333333-3333-4333-8333-333333333333".to_owned(),
            row_version: 0,
            summary: "workspace_write_file (4 bytes)".to_owned(),
            target: "../outside.txt".to_owned(),
        };
        assert!(unsafe_target.validate().is_err());
    }

    #[test]
    fn approval_contract_rejects_payload_fields() {
        assert!(
            serde_json::from_str::<ServiceApprovalPreviewV1>(
                r#"{"wait_id":"33333333-3333-4333-8333-333333333333","row_version":1,"summary":"workspace_write_file (4 bytes)","target":"safe.txt","content":"secret"}"#,
            )
            .is_err()
        );
        assert!(
            serde_json::from_str::<WaitingPageV1>(
                r#"{"items":[],"next_run_id":null,"next_wait_id":null,"capsule":"secret"}"#,
            )
            .is_err()
        );
    }
}
