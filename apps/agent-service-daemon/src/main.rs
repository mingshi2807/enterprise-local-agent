use std::{
    env,
    fs::{File, OpenOptions, Permissions},
    io::Write,
    os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use agent_core::{AgentEvent, ModelRequest, ModelResponse, RunBudget};
use agent_harness::{
    AuditFailurePolicy, AuditPortError, AuditSink, ExecutionHarness, HarnessConfig,
    M0ReadOnlyPolicy, ModelPort, ModelPortError, PortFuture, RecoveredRun, RecoveredWaitingRun,
    RecoveryContract, RunContext, RunKey, ToolRegistry,
};
use agent_persistence_sqlite::SqliteRunPersistence;
use agent_service::{
    AgentService, ConfiguredWorkflow, RunInput, ServiceFuture, WorkflowError, WorkflowId,
};
use agent_service_http::{HttpSecurity, router};
use anyhow::{Context, bail};
use rustix::fs::{FlockOperation, flock};

const DATA_DIR_ENV: &str = "ELA_SERVICE_DATA_DIR";
const LOOPBACK_ADDR_ENV: &str = "ELA_SERVICE_LOOPBACK_ADDR";
const BEARER_ENV: &str = "ELA_SERVICE_BEARER";
const HOSTS_ENV: &str = "ELA_SERVICE_ALLOWED_HOSTS";
const ORIGINS_ENV: &str = "ELA_SERVICE_ALLOWED_ORIGINS";

struct UnavailableModel;

impl ModelPort for UnavailableModel {
    fn invoke<'a>(
        &'a self,
        _request: ModelRequest,
    ) -> PortFuture<'a, Result<ModelResponse, ModelPortError>> {
        Box::pin(async { Err(ModelPortError::Unavailable) })
    }
}

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
    ) -> ServiceFuture<'static, Result<(), WorkflowError>> {
        Box::pin(async move {
            harness
                .complete_run(&mut context)
                .await
                .map_err(|_| WorkflowError::Failed)
        })
    }
    fn resume_waiting(
        self: Arc<Self>,
        _harness: Arc<ExecutionHarness>,
        _recovered: RecoveredWaitingRun,
        _wait_id: agent_core::DurableApprovalWaitId,
    ) -> ServiceFuture<'static, Result<(), WorkflowError>> {
        Box::pin(async { Err(WorkflowError::NotRestartable) })
    }
    fn resume_recovered(
        self: Arc<Self>,
        _harness: Arc<ExecutionHarness>,
        _recovered: RecoveredRun,
    ) -> ServiceFuture<'static, Result<(), WorkflowError>> {
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
    let harness = Arc::new(
        ExecutionHarness::new(
            Arc::new(UnavailableModel),
            ToolRegistry::new(),
            Arc::new(M0ReadOnlyPolicy),
            audit,
            HarnessConfig::new(AuditFailurePolicy::FailClosed, Duration::from_secs(1))?,
        )
        .with_persistence_port(persistence.clone()),
    );
    let workflow: Arc<dyn ConfiguredWorkflow> = Arc::new(HealthWorkflow::new()?);
    let service = Arc::new(AgentService::new(harness, persistence, vec![workflow])?);
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
}
