use std::{
    env,
    fs::{File, OpenOptions, Permissions},
    io::Write,
    os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use agent_action_seal_local::LocalActionSealer;
use agent_containment_linux::{LinuxContainmentConfig, LinuxWorkspaceWriteTool};
use agent_core::{AgentEvent, RunBudget, WorkspaceBindingId};
use agent_harness::{
    AuditFailurePolicy, AuditPortError, AuditSink, ExecutionHarness, HarnessConfig,
    M6ApprovalPolicy, PortFuture, RecoveredRun, RecoveredWaitingRun, RecoveryContract, RunContext,
    RunKey, ToolRegistry,
};
use agent_knowledge::{
    FederatedFailurePolicy, KnowledgeBackendPort, KnowledgePort, KnowledgeQuery, KnowledgeRequest,
    KnowledgeRoute, RetrievalLimits, RoutedKnowledgePort,
};
use agent_knowledge_adapters::{
    OcppApiConfig, OcppKnowledgeAdapter, StandardsMcpConfig, StandardsMcpKnowledgeAdapter,
};
use agent_mvp::EnterpriseMvpWorkflow;
use agent_persistence_sqlite::SqliteRunPersistence;
use agent_provider_rig::{
    BearerCredential, OpenAiCompatibleConfig, ProviderLabel, build_openai_compatible_model_port,
    probe_openai_compatible,
};
use agent_service::{
    AgentService, ConfiguredWorkflow, RunInput, ServiceFuture, WorkflowCompletion, WorkflowError,
    WorkflowId,
};
use agent_service_http::{HttpSecurity, router};
use anyhow::{Context, bail};
use rustix::fs::{FlockOperation, flock};

const DATA_DIR_ENV: &str = "ELA_SERVICE_DATA_DIR";
const LOOPBACK_ADDR_ENV: &str = "ELA_SERVICE_LOOPBACK_ADDR";
const BEARER_ENV: &str = "ELA_SERVICE_BEARER";
const HOSTS_ENV: &str = "ELA_SERVICE_ALLOWED_HOSTS";
const ORIGINS_ENV: &str = "ELA_SERVICE_ALLOWED_ORIGINS";
const MODEL_URL_ENV: &str = "ELA_MODEL_BASE_URL";
const MODEL_ID_ENV: &str = "ELA_MODEL_ID";
const MODEL_PROVIDER_ENV: &str = "ELA_MODEL_PROVIDER_LABEL";
const MODEL_AUTH_ENV: &str = "ELA_MODEL_AUTH";
const MODEL_BEARER_ENV: &str = "ELA_MODEL_BEARER";
const KNOWLEDGE_ROUTE_ENV: &str = "ELA_KNOWLEDGE_ROUTE";
const OCPP_URL_ENV: &str = "ELA_OCPP_KNOWLEDGE_URL";
const STANDARDS_EXE_ENV: &str = "ELA_STANDARDS_MCP_EXECUTABLE";
const STANDARDS_ARGV_ENV: &str = "ELA_STANDARDS_MCP_ARGV_JSON";
const STANDARDS_ENV_ENV: &str = "ELA_STANDARDS_MCP_ENV_JSON";
const WORKSPACE_ENV: &str = "ELA_LOCALWRITE_WORKSPACE";
const BWRAP_ENV: &str = "ELA_BWRAP_PATH";
const WORKER_ENV: &str = "ELA_LOCALWRITE_WORKER_PATH";
const SEAL_KEY_ID_ENV: &str = "ELA_ACTION_SEAL_KEY_ID";
const SEAL_KEY_HEX_ENV: &str = "ELA_ACTION_SEAL_KEY_HEX";
const WORKSPACE_BINDING_ENV: &str = "ELA_WORKSPACE_BINDING_ID";

struct MetadataAudit {
    file: Arc<std::sync::Mutex<File>>,
}

impl MetadataAudit {
    fn open(path: &Path) -> anyhow::Result<Self> {
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .context("failed to open metadata audit")?;
        std::fs::set_permissions(path, Permissions::from_mode(0o600))
            .context("failed to restrict metadata audit")?;
        Ok(Self {
            file: Arc::new(std::sync::Mutex::new(file)),
        })
    }
}

impl AuditSink for MetadataAudit {
    fn record<'a>(&'a self, event: &'a AgentEvent) -> PortFuture<'a, Result<(), AuditPortError>> {
        let file = self.file.clone();
        let event = event.clone();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || {
                let bytes = serde_json::to_vec(&event).map_err(|_| AuditPortError::RecordFailed)?;
                let mut file = file.lock().map_err(|_| AuditPortError::RecordFailed)?;
                file.write_all(&bytes)
                    .and_then(|_| file.write_all(b"\n"))
                    .and_then(|_| file.sync_data())
                    .map_err(|_| AuditPortError::RecordFailed)
            })
            .await
            .map_err(|_| AuditPortError::RecordFailed)?
        })
    }
}

struct HealthWorkflow {
    id: WorkflowId,
    budget: RunBudget,
}

impl HealthWorkflow {
    fn new() -> anyhow::Result<Self> {
        Ok(Self {
            id: WorkflowId::new("service-health")?,
            budget: RunBudget::new(1, 1, 1, Duration::from_secs(30))?,
        })
    }
}

impl ConfiguredWorkflow for HealthWorkflow {
    fn id(&self) -> &WorkflowId {
        &self.id
    }
    fn recovery_contract(&self) -> RecoveryContract {
        RecoveryContract::NonRestartable
    }
    fn new_context(&self, key: RunKey) -> RunContext {
        RunContext::new(key.run_id(), key.session_id(), self.budget)
    }
    fn run(
        self: Arc<Self>,
        harness: Arc<ExecutionHarness>,
        mut context: RunContext,
        _input: RunInput,
    ) -> ServiceFuture<'static, Result<WorkflowCompletion, WorkflowError>> {
        Box::pin(async move {
            harness
                .complete_run(&mut context)
                .await
                .map(|()| WorkflowCompletion::NoApplicationResult)
                .map_err(|_| WorkflowError::Failed)
        })
    }
    fn resume_waiting(
        self: Arc<Self>,
        _harness: Arc<ExecutionHarness>,
        _recovered: RecoveredWaitingRun,
        _wait_id: agent_core::DurableApprovalWaitId,
    ) -> ServiceFuture<'static, Result<WorkflowCompletion, WorkflowError>> {
        Box::pin(async { Err(WorkflowError::NotRestartable) })
    }
    fn resume_recovered(
        self: Arc<Self>,
        _harness: Arc<ExecutionHarness>,
        _recovered: RecoveredRun,
    ) -> ServiceFuture<'static, Result<WorkflowCompletion, WorkflowError>> {
        Box::pin(async { Err(WorkflowError::NotRestartable) })
    }
}

struct DataDirectory {
    path: PathBuf,
    _lock: File,
}

impl DataDirectory {
    fn acquire(path: PathBuf) -> anyhow::Result<Self> {
        if !path.is_absolute() {
            bail!("{DATA_DIR_ENV} must be an absolute path");
        }
        std::fs::create_dir_all(&path).context("failed to create service data directory")?;
        let metadata =
            std::fs::symlink_metadata(&path).context("failed to inspect service data directory")?;
        if !metadata.file_type().is_dir() || metadata.uid() != rustix::process::geteuid().as_raw() {
            bail!("service data directory is not an owned directory");
        }
        std::fs::set_permissions(&path, Permissions::from_mode(0o700))
            .context("failed to restrict service data directory")?;
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(path.join("service.lock"))
            .context("failed to open service lock")?;
        lock.set_permissions(Permissions::from_mode(0o600))
            .context("failed to restrict service lock")?;
        flock(&lock, FlockOperation::NonBlockingLockExclusive)
            .map_err(|_| anyhow::anyhow!("service data directory is already owned"))?;
        Ok(Self { path, _lock: lock })
    }
    fn database(&self) -> PathBuf {
        self.path.join("runs.sqlite3")
    }
    fn socket(&self) -> PathBuf {
        self.path.join("agent-service.sock")
    }
    fn audit(&self) -> PathBuf {
        self.path.join("audit.jsonl")
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_target(false)
        .without_time()
        .try_init()
        .map_err(|_| anyhow::anyhow!("failed to initialize tracing"))?;
    let data_dir = env::var_os(DATA_DIR_ENV)
        .map(PathBuf::from)
        .context("ELA_SERVICE_DATA_DIR is required")?;
    let data = DataDirectory::acquire(data_dir)?;
    let persistence = Arc::new(SqliteRunPersistence::open(data.database()).await?);
    let audit = Arc::new(MetadataAudit::open(&data.audit())?);
    let model_config = model_config()?;
    probe_openai_compatible(&model_config)
        .await
        .context("configured model readiness failed")?;
    let model = build_openai_compatible_model_port(model_config)
        .context("failed to construct configured model")?;
    let (knowledge, route) = knowledge_config()?;
    tokio::time::timeout(
        Duration::from_secs(30),
        knowledge.retrieve(KnowledgeRequest::new(
            KnowledgeQuery::new("enterprise knowledge readiness")?,
            route.clone(),
            RetrievalLimits::new(1, 512, 512)?,
        )),
    )
    .await
    .context("configured knowledge readiness timed out")?
    .context("configured knowledge readiness failed")?;

    let mut tools = ToolRegistry::new();
    let localwrite = match localwrite_config().await {
        Ok(value) => value,
        Err(_) => {
            tracing::warn!("LocalWrite workflow readiness failed; workflow not registered");
            None
        }
    };
    let localwrite_enabled = localwrite.is_some();
    let localwrite_ready = if let Some((tool, sealer, workspace_binding)) = localwrite {
        let port: Arc<dyn agent_harness::ContainedToolPort> = tool;
        tools.register_contained(port)?;
        Some((sealer, workspace_binding))
    } else {
        None
    };
    let mut harness = ExecutionHarness::new(
        model,
        tools,
        Arc::new(M6ApprovalPolicy),
        audit,
        HarnessConfig::new(AuditFailurePolicy::FailClosed, Duration::from_secs(10))?,
    )
    .with_persistence_port(persistence.clone())
    .with_knowledge_port(knowledge);
    if let Some((sealer, workspace_binding)) = localwrite_ready {
        harness = harness.with_durable_local_write_approval(sealer, workspace_binding);
    }
    let harness = Arc::new(harness);
    let workflows = workflow_catalog(route, localwrite_enabled)?;
    let service = Arc::new(AgentService::new(harness, persistence, workflows)?);
    let discovered = service.discover_runs().await?;
    tracing::info!(
        durable_runs = discovered.len(),
        "durable run catalog loaded"
    );

    if let Ok(address) = env::var(LOOPBACK_ADDR_ENV) {
        let address: std::net::SocketAddr = address.parse().context("invalid loopback address")?;
        if !address.ip().is_loopback() {
            bail!("non-loopback service listener is forbidden");
        }
        let bearer = env::var(BEARER_ENV).context("loopback bearer is required")?;
        let hosts = split_required(HOSTS_ENV)?;
        let origins = split_required(ORIGINS_ENV)?;
        let listener = tokio::net::TcpListener::bind(address).await?;
        return axum::serve(
            listener,
            router(
                service,
                HttpSecurity::loopback(&bearer, hosts, origins)
                    .map_err(|_| anyhow::anyhow!("invalid loopback security configuration"))?,
            ),
        )
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("service failed");
    }

    let socket = data.socket();
    prepare_socket(&socket)?;
    let listener =
        tokio::net::UnixListener::bind(&socket).context("failed to bind service socket")?;
    std::fs::set_permissions(&socket, Permissions::from_mode(0o600))
        .context("failed to restrict service socket")?;
    let result = axum::serve(listener, router(service, HttpSecurity::unix_socket()))
        .with_graceful_shutdown(shutdown_signal())
        .await;
    let _ = std::fs::remove_file(&socket);
    result.context("service failed")
}

fn model_config() -> anyhow::Result<OpenAiCompatibleConfig> {
    let base = env::var(MODEL_URL_ENV).context("ELA_MODEL_BASE_URL is required")?;
    let model = env::var(MODEL_ID_ENV).context("ELA_MODEL_ID is required")?;
    let label = ProviderLabel::new(
        env::var(MODEL_PROVIDER_ENV).context("ELA_MODEL_PROVIDER_LABEL is required")?,
    )?;
    let config = match env::var(MODEL_AUTH_ENV).as_deref() {
        Ok("bearer") => OpenAiCompatibleConfig::new(
            base,
            model,
            BearerCredential::new(
                env::var(MODEL_BEARER_ENV).context("ELA_MODEL_BEARER is required")?,
            )?,
        )?,
        Ok("no-auth-loopback") => OpenAiCompatibleConfig::new_no_auth_loopback(base, model)?,
        _ => bail!("ELA_MODEL_AUTH must be bearer or no-auth-loopback"),
    };
    Ok(config.with_provider_label(label))
}

fn knowledge_config() -> anyhow::Result<(Arc<dyn KnowledgePort>, KnowledgeRoute)> {
    let route_name = env::var(KNOWLEDGE_ROUTE_ENV).context("ELA_KNOWLEDGE_ROUTE is required")?;
    let mut backends: Vec<Arc<dyn KnowledgeBackendPort>> = Vec::new();
    let route = match route_name.as_str() {
        "ocpp" => {
            backends.push(ocpp_backend()?);
            KnowledgeRoute::single(agent_core::KnowledgeBackendId::OcppRagKag)
        }
        "standards" => {
            backends.push(standards_backend()?);
            KnowledgeRoute::single(agent_core::KnowledgeBackendId::StandardsMcp)
        }
        "federated" | "federated-partial" => {
            backends.push(ocpp_backend()?);
            backends.push(standards_backend()?);
            KnowledgeRoute::federated(
                vec![
                    agent_core::KnowledgeBackendId::OcppRagKag,
                    agent_core::KnowledgeBackendId::StandardsMcp,
                ],
                if route_name == "federated-partial" {
                    FederatedFailurePolicy::AllowPartial
                } else {
                    FederatedFailurePolicy::RequireAll
                },
            )?
        }
        _ => bail!("ELA_KNOWLEDGE_ROUTE is invalid"),
    };
    Ok((Arc::new(RoutedKnowledgePort::new(backends)?), route))
}

fn ocpp_backend() -> anyhow::Result<Arc<dyn KnowledgeBackendPort>> {
    let url =
        url::Url::parse(&env::var(OCPP_URL_ENV).context("ELA_OCPP_KNOWLEDGE_URL is required")?)?;
    let config = OcppApiConfig::new(url)
        .map_err(|_| anyhow::anyhow!("OCPP knowledge configuration is invalid"))?;
    let adapter = OcppKnowledgeAdapter::new(config)
        .map_err(|_| anyhow::anyhow!("OCPP knowledge adapter construction failed"))?;
    Ok(Arc::new(adapter))
}

fn standards_backend() -> anyhow::Result<Arc<dyn KnowledgeBackendPort>> {
    let executable = PathBuf::from(
        env::var_os(STANDARDS_EXE_ENV).context("ELA_STANDARDS_MCP_EXECUTABLE is required")?,
    );
    let arguments: Vec<String> =
        serde_json::from_str(&env::var(STANDARDS_ARGV_ENV).unwrap_or_else(|_| "[]".to_owned()))
            .context("ELA_STANDARDS_MCP_ARGV_JSON is invalid")?;
    let environment: Vec<(String, String)> =
        serde_json::from_str(&env::var(STANDARDS_ENV_ENV).unwrap_or_else(|_| "[]".to_owned()))
            .context("ELA_STANDARDS_MCP_ENV_JSON is invalid")?;
    let config = StandardsMcpConfig::new(
        executable,
        arguments.into_iter().map(Into::into).collect(),
        environment
            .into_iter()
            .map(|(key, value)| (key.into(), value.into()))
            .collect(),
    )
    .map_err(|_| anyhow::anyhow!("standards MCP configuration is invalid"))?;
    Ok(Arc::new(StandardsMcpKnowledgeAdapter::new(config)))
}

fn localwrite_configured() -> bool {
    [
        WORKSPACE_ENV,
        BWRAP_ENV,
        WORKER_ENV,
        SEAL_KEY_ID_ENV,
        SEAL_KEY_HEX_ENV,
        WORKSPACE_BINDING_ENV,
    ]
    .iter()
    .all(|name| env::var_os(name).is_some())
}

async fn localwrite_config() -> anyhow::Result<
    Option<(
        Arc<LinuxWorkspaceWriteTool>,
        Arc<LocalActionSealer>,
        WorkspaceBindingId,
    )>,
> {
    let configured = [
        WORKSPACE_ENV,
        BWRAP_ENV,
        WORKER_ENV,
        SEAL_KEY_ID_ENV,
        SEAL_KEY_HEX_ENV,
        WORKSPACE_BINDING_ENV,
    ]
    .iter()
    .filter(|name| env::var_os(name).is_some())
    .count();
    if configured == 0 {
        return Ok(None);
    }
    if !localwrite_configured() {
        bail!("LocalWrite configuration is incomplete");
    }
    let tool = Arc::new(
        LinuxWorkspaceWriteTool::probe_and_create(LinuxContainmentConfig::new(
            PathBuf::from(env::var_os(WORKSPACE_ENV).context("workspace is required")?),
            PathBuf::from(env::var_os(BWRAP_ENV).context("bwrap path is required")?),
            PathBuf::from(env::var_os(WORKER_ENV).context("worker path is required")?),
        ))
        .await?,
    );
    let key = decode_key(&env::var(SEAL_KEY_HEX_ENV).context("seal key is required")?)?;
    let sealer = Arc::new(LocalActionSealer::new(env::var(SEAL_KEY_ID_ENV)?, key)?);
    let binding = env::var(WORKSPACE_BINDING_ENV)?.parse::<WorkspaceBindingId>()?;
    Ok(Some((tool, sealer, binding)))
}

fn workflow_catalog(
    route: KnowledgeRoute,
    localwrite_ready: bool,
) -> anyhow::Result<Vec<Arc<dyn ConfiguredWorkflow>>> {
    let mut workflows: Vec<Arc<dyn ConfiguredWorkflow>> = vec![
        Arc::new(HealthWorkflow::new()?),
        Arc::new(EnterpriseMvpWorkflow::readonly(route.clone())?),
    ];
    if localwrite_ready {
        workflows.push(Arc::new(EnterpriseMvpWorkflow::localwrite(route)?));
    }
    Ok(workflows)
}

fn decode_key(value: &str) -> anyhow::Result<[u8; 32]> {
    if value.len() != 64 {
        bail!("action seal key must be 32-byte hexadecimal");
    }
    let mut key = [0_u8; 32];
    for (index, slot) in key.iter_mut().enumerate() {
        *slot = u8::from_str_radix(&value[index * 2..index * 2 + 2], 16)
            .map_err(|_| anyhow::anyhow!("action seal key must be hexadecimal"))?;
    }
    Ok(key)
}

fn split_required(name: &str) -> anyhow::Result<Vec<String>> {
    let values = env::var(name)
        .with_context(|| format!("{name} is required"))?
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .collect::<Vec<_>>();
    if values.is_empty() {
        bail!("{name} must not be empty");
    }
    Ok(values)
}

fn prepare_socket(path: &Path) -> anyhow::Result<()> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error).context("failed to inspect service socket"),
    };
    if !metadata.file_type().is_socket() || metadata.uid() != rustix::process::geteuid().as_raw() {
        bail!("service socket path is not an owned socket");
    }
    std::fs::remove_file(path).context("failed to remove stale service socket")
}

async fn shutdown_signal() {
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut signal) => {
                signal.recv().await;
            }
            Err(_) => std::future::pending::<()>().await,
        }
    };
    tokio::select! {
        result = tokio::signal::ctrl_c() => {
            let _ = result;
        }
        () = terminate => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_directory_has_one_process_owner() {
        let directory = tempfile::tempdir().expect("directory");
        let path = directory.path().join("service-data");
        let first = DataDirectory::acquire(path.clone()).expect("first owner");
        assert!(DataDirectory::acquire(path.clone()).is_err());
        drop(first);
        DataDirectory::acquire(path).expect("lock released when owner exits");
    }

    #[test]
    fn stale_owned_socket_is_removed_but_other_objects_are_rejected() {
        let directory = tempfile::tempdir().expect("directory");
        let socket = directory.path().join("service.sock");
        let listener = std::os::unix::net::UnixListener::bind(&socket).expect("bind socket");
        drop(listener);
        prepare_socket(&socket).expect("remove stale socket");
        assert!(!socket.exists());

        std::fs::write(&socket, b"not a socket").expect("write marker");
        assert!(prepare_socket(&socket).is_err());
    }

    #[test]
    fn readonly_registration_does_not_depend_on_localwrite_readiness() {
        let route = KnowledgeRoute::single(agent_core::KnowledgeBackendId::OcppRagKag);
        let readonly = workflow_catalog(route.clone(), false).expect("readonly catalog");
        assert!(
            readonly
                .iter()
                .any(|workflow| workflow.id().as_str() == agent_mvp::READONLY_WORKFLOW_ID)
        );
        assert!(
            !readonly
                .iter()
                .any(|workflow| workflow.id().as_str() == agent_mvp::LOCALWRITE_WORKFLOW_ID)
        );

        let with_write = workflow_catalog(route, true).expect("localwrite catalog");
        assert!(
            with_write
                .iter()
                .any(|workflow| workflow.id().as_str() == agent_mvp::LOCALWRITE_WORKFLOW_ID)
        );
    }
}
