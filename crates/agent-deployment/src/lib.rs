//! Deployment configuration and offline operations for the governed runtime.

use std::{
    fmt,
    fs::Permissions,
    fs::{File, OpenOptions},
    io::{Read, Write},
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use agent_core::WorkspaceBindingId;
use agent_harness::RecoveryContract;
use agent_identity::{ApprovalSeparation, PrincipalKind, PrincipalRole};
use agent_knowledge::{FederatedFailurePolicy, KnowledgeRoute};
use agent_mvp::EnterpriseMvpWorkflow;
use agent_persistence_sqlite::{SqliteStoreAdmin, StoreInspection};
use agent_service::ConfiguredWorkflow;
use agent_service::ServiceObserver;
use rustix::fs::{FlockOperation, flock};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;
use url::Url;
use zeroize::Zeroize;

pub const DEPLOYMENT_CONFIG_SCHEMA_VERSION: u16 = 2;
pub const MAX_DEPLOYMENT_CONFIG_BYTES: usize = 64 * 1024;
pub const BACKUP_MANIFEST_VERSION: u16 = 1;
const MAX_SECRET_BYTES: usize = 16 * 1024;
const MAX_TEXT_BYTES: usize = 1024;
const MAX_LIST_ITEMS: usize = 32;
const MAX_MCP_SERVERS: usize = 16;

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeploymentConfigV2 {
    pub schema_version: u16,
    pub listener: ListenerConfigV1,
    pub identity: IdentityConfigV1,
    pub storage: StorageConfigV1,
    pub workflows: WorkflowConfigV1,
    pub model: ModelConfigV1,
    pub knowledge: KnowledgeConfigV1,
    #[serde(default)]
    pub mcp: Vec<McpServerConfigV1>,
    pub local_write: Option<LocalWriteConfigV1>,
    #[serde(default)]
    pub limits: OperationalLimitsV1,
}

impl fmt::Debug for DeploymentConfigV2 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DeploymentConfigV2")
            .field("schema_version", &self.schema_version)
            .field("listener", &self.listener)
            .field("identity", &self.identity)
            .field("storage", &self.storage)
            .field("workflows", &self.workflows)
            .field("model", &self.model)
            .field("knowledge", &self.knowledge)
            .field("mcp_count", &self.mcp.len())
            .field("local_write", &self.local_write)
            .field("limits", &self.limits)
            .finish()
    }
}

impl DeploymentConfigV2 {
    pub fn parse(bytes: &[u8]) -> Result<Self, DeploymentConfigError> {
        if bytes.is_empty() || bytes.len() > MAX_DEPLOYMENT_CONFIG_BYTES {
            return Err(DeploymentConfigError::InvalidConfig);
        }
        let text = std::str::from_utf8(bytes).map_err(|_| DeploymentConfigError::InvalidConfig)?;
        let config: Self =
            toml::from_str(text).map_err(|_| DeploymentConfigError::InvalidConfig)?;
        config.validate()?;
        Ok(config)
    }

    pub fn load(path: &Path) -> Result<Self, DeploymentConfigError> {
        let metadata = std::fs::symlink_metadata(path)
            .map_err(|_| DeploymentConfigError::ConfigurationUnavailable)?;
        if !path.is_absolute() || !metadata.file_type().is_file() {
            return Err(DeploymentConfigError::ConfigurationUnavailable);
        }
        let bytes =
            std::fs::read(path).map_err(|_| DeploymentConfigError::ConfigurationUnavailable)?;
        Self::parse(&bytes)
    }

    pub fn validate(&self) -> Result<(), DeploymentConfigError> {
        if self.schema_version != DEPLOYMENT_CONFIG_SCHEMA_VERSION
            || self.mcp.len() > MAX_MCP_SERVERS
            || !self.workflows.readonly_enabled
            || self.workflows.local_write_enabled != self.local_write.is_some()
        {
            return Err(DeploymentConfigError::InvalidConfig);
        }
        self.listener.validate()?;
        self.identity.validate(&self.listener)?;
        self.storage.validate()?;
        self.model.validate()?;
        self.knowledge.validate()?;
        self.limits.validate()?;
        for server in &self.mcp {
            server.validate()?;
        }
        if let Some(local_write) = &self.local_write {
            local_write.validate()?;
        }
        Ok(())
    }

    pub fn fingerprint(&self) -> Result<DeploymentFingerprint, DeploymentConfigError> {
        self.validate()?;
        let encoded = serde_json::to_vec(self).map_err(|_| DeploymentConfigError::InvalidConfig)?;
        let mut hasher = Sha256::new();
        hasher.update(b"enterprise-local-agent/deployment-config/v1\0");
        hasher.update(encoded);
        Ok(DeploymentFingerprint(hasher.finalize().into()))
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IdentityConfigV1 {
    pub policy_version: u32,
    pub approval_separation: ApprovalSeparation,
    pub principals: Vec<PrincipalConfigV1>,
}

impl IdentityConfigV1 {
    fn validate(&self, listener: &ListenerConfigV1) -> Result<(), DeploymentConfigError> {
        if self.policy_version == 0 || self.principals.is_empty() || self.principals.len() > 32 {
            return Err(DeploymentConfigError::InvalidConfig);
        }
        for (index, principal) in self.principals.iter().enumerate() {
            principal.validate()?;
            if self.principals[..index].iter().any(|existing| {
                existing.principal_id == principal.principal_id
                    || principal.unix_uid.zip(principal.unix_gid).is_some()
                        && existing.unix_uid.zip(existing.unix_gid)
                            == principal.unix_uid.zip(principal.unix_gid)
            }) {
                return Err(DeploymentConfigError::InvalidConfig);
            }
        }
        match listener {
            ListenerConfigV1::Unix { .. } => {
                if !self
                    .principals
                    .iter()
                    .any(|principal| principal.unix_uid.is_some() && principal.unix_gid.is_some())
                {
                    return Err(DeploymentConfigError::InvalidConfig);
                }
            }
            ListenerConfigV1::LoopbackTcp { .. } => {
                if self
                    .principals
                    .iter()
                    .filter(|principal| principal.loopback_bearer)
                    .count()
                    != 1
                {
                    return Err(DeploymentConfigError::InvalidConfig);
                }
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PrincipalConfigV1 {
    pub principal_id: agent_core::PrincipalId,
    pub kind: PrincipalKind,
    pub roles: Vec<PrincipalRole>,
    pub unix_uid: Option<u32>,
    pub unix_gid: Option<u32>,
    #[serde(default)]
    pub loopback_bearer: bool,
}

impl PrincipalConfigV1 {
    fn validate(&self) -> Result<(), DeploymentConfigError> {
        if self.roles.is_empty()
            || self.roles.len() > agent_identity::MAX_PRINCIPAL_ROLES
            || self
                .roles
                .iter()
                .enumerate()
                .any(|(index, role)| self.roles[..index].contains(role))
            || self.unix_uid.is_some() != self.unix_gid.is_some()
        {
            return Err(DeploymentConfigError::InvalidConfig);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ListenerConfigV1 {
    Unix {
        socket_path: PathBuf,
    },
    LoopbackTcp {
        address: String,
        bearer: SecretReferenceV1,
        allowed_hosts: Vec<String>,
        allowed_origins: Vec<String>,
    },
}

impl ListenerConfigV1 {
    fn validate(&self) -> Result<(), DeploymentConfigError> {
        match self {
            Self::Unix { socket_path } => require_absolute(socket_path),
            Self::LoopbackTcp {
                address,
                bearer,
                allowed_hosts,
                allowed_origins,
            } => {
                let address = address
                    .parse::<std::net::SocketAddr>()
                    .map_err(|_| DeploymentConfigError::InvalidConfig)?;
                if !address.ip().is_loopback()
                    || !valid_nonempty_list(allowed_hosts)
                    || !valid_nonempty_list(allowed_origins)
                    || allowed_hosts
                        .iter()
                        .chain(allowed_origins)
                        .any(|v| v == "*")
                {
                    return Err(DeploymentConfigError::InvalidConfig);
                }
                bearer.validate()
            }
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StorageConfigV1 {
    pub data_directory: PathBuf,
    pub database_file: String,
    pub audit_file: String,
}

impl StorageConfigV1 {
    fn validate(&self) -> Result<(), DeploymentConfigError> {
        require_absolute(&self.data_directory)?;
        require_file_name(&self.database_file)?;
        require_file_name(&self.audit_file)
    }

    #[must_use]
    pub fn database_path(&self) -> PathBuf {
        self.data_directory.join(&self.database_file)
    }

    #[must_use]
    pub fn audit_path(&self) -> PathBuf {
        self.data_directory.join(&self.audit_file)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowConfigV1 {
    pub readonly_enabled: bool,
    pub readonly_required: bool,
    pub local_write_enabled: bool,
    pub local_write_required: bool,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ModelAuthConfigV1 {
    Bearer { credential: SecretReferenceV1 },
    NoAuthLoopback,
}

impl fmt::Debug for ModelAuthConfigV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Bearer { .. } => formatter.write_str("Bearer([REDACTED])"),
            Self::NoAuthLoopback => formatter.write_str("NoAuthLoopback"),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelConfigV1 {
    pub base_url: String,
    pub model_id: String,
    pub provider_label: String,
    pub auth: ModelAuthConfigV1,
    #[serde(default)]
    pub disable_reasoning: bool,
    #[serde(default = "default_model_probe_timeout_ms")]
    pub readiness_timeout_ms: u64,
}

impl ModelConfigV1 {
    fn validate(&self) -> Result<(), DeploymentConfigError> {
        bounded_text(&self.model_id)?;
        bounded_text(&self.provider_label)?;
        bounded_duration(self.readiness_timeout_ms)?;
        let url = Url::parse(&self.base_url).map_err(|_| DeploymentConfigError::InvalidConfig)?;
        if !matches!(url.scheme(), "http" | "https") {
            return Err(DeploymentConfigError::InvalidConfig);
        }
        match &self.auth {
            ModelAuthConfigV1::Bearer { credential } => credential.validate(),
            ModelAuthConfigV1::NoAuthLoopback => {
                let loopback = url.host_str().is_some_and(|host| {
                    host == "localhost"
                        || host
                            .parse::<std::net::IpAddr>()
                            .is_ok_and(|ip| ip.is_loopback())
                });
                loopback
                    .then_some(())
                    .ok_or(DeploymentConfigError::InvalidConfig)
            }
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "route", rename_all = "snake_case", deny_unknown_fields)]
pub enum KnowledgeConfigV1 {
    Ocpp {
        base_url: String,
        readiness_query: String,
    },
    Standards {
        executable: PathBuf,
        #[serde(default)]
        arguments: Vec<String>,
        #[serde(default)]
        environment: Vec<EnvironmentEntryV1>,
        readiness_query: String,
    },
    Federated {
        ocpp_base_url: String,
        standards_executable: PathBuf,
        #[serde(default)]
        standards_arguments: Vec<String>,
        #[serde(default)]
        standards_environment: Vec<EnvironmentEntryV1>,
        allow_partial: bool,
        readiness_query: String,
    },
}

impl KnowledgeConfigV1 {
    fn validate(&self) -> Result<(), DeploymentConfigError> {
        match self {
            Self::Ocpp {
                base_url,
                readiness_query,
            } => {
                validate_http_url(base_url)?;
                bounded_text(readiness_query)
            }
            Self::Standards {
                executable,
                arguments,
                environment,
                readiness_query,
            } => {
                validate_process(executable, arguments, environment)?;
                bounded_text(readiness_query)
            }
            Self::Federated {
                ocpp_base_url,
                standards_executable,
                standards_arguments,
                standards_environment,
                readiness_query,
                ..
            } => {
                validate_http_url(ocpp_base_url)?;
                validate_process(
                    standards_executable,
                    standards_arguments,
                    standards_environment,
                )?;
                bounded_text(readiness_query)
            }
        }
    }

    #[must_use]
    pub fn readiness_query(&self) -> &str {
        match self {
            Self::Ocpp {
                readiness_query, ..
            }
            | Self::Standards {
                readiness_query, ..
            }
            | Self::Federated {
                readiness_query, ..
            } => readiness_query,
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnvironmentEntryV1 {
    pub name: String,
    pub literal: Option<String>,
    pub secret: Option<SecretReferenceV1>,
}

impl fmt::Debug for EnvironmentEntryV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("EnvironmentEntryV1")
            .field("name", &self.name)
            .field("value", &"[REDACTED]")
            .finish()
    }
}

impl EnvironmentEntryV1 {
    fn validate(&self) -> Result<(), DeploymentConfigError> {
        if !valid_environment_name(&self.name) || self.literal.is_some() == self.secret.is_some() {
            return Err(DeploymentConfigError::InvalidConfig);
        }
        if let Some(value) = &self.literal {
            bounded_text(value)?;
        }
        if let Some(secret) = &self.secret {
            secret.validate()?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct McpServerConfigV1 {
    pub server_id: String,
    pub executable: PathBuf,
    #[serde(default)]
    pub arguments: Vec<String>,
    #[serde(default)]
    pub environment: Vec<EnvironmentEntryV1>,
    pub protocol_revision: String,
    pub definition_fingerprint: String,
    pub enabled: bool,
}

impl McpServerConfigV1 {
    fn validate(&self) -> Result<(), DeploymentConfigError> {
        bounded_text(&self.server_id)?;
        bounded_text(&self.protocol_revision)?;
        require_sha256(&self.definition_fingerprint)?;
        validate_process(&self.executable, &self.arguments, &self.environment)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocalWriteConfigV1 {
    pub workspace: PathBuf,
    pub workspace_binding_id: WorkspaceBindingId,
    pub seal_key_id: String,
    pub seal_key: SecretReferenceV1,
    pub bwrap_path: PathBuf,
    pub bwrap_sha256: String,
    pub worker_path: PathBuf,
    pub worker_sha256: String,
    pub worker_protocol_version: u16,
    pub tool_contract_sha256: String,
}

impl LocalWriteConfigV1 {
    fn validate(&self) -> Result<(), DeploymentConfigError> {
        require_absolute(&self.workspace)?;
        require_absolute(&self.bwrap_path)?;
        require_absolute(&self.worker_path)?;
        bounded_text(&self.seal_key_id)?;
        self.seal_key.validate()?;
        require_sha256(&self.bwrap_sha256)?;
        require_sha256(&self.worker_sha256)?;
        if self.worker_protocol_version != agent_containment_linux::LOCAL_WRITE_PROTOCOL_VERSION {
            return Err(DeploymentConfigError::InvalidConfig);
        }
        require_sha256(&self.tool_contract_sha256)
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct OperationalLimitsV1 {
    pub max_active_runs: u16,
    pub max_connections: u16,
    pub shutdown_grace_seconds: u64,
    pub readiness_cache_seconds: u64,
}

impl Default for OperationalLimitsV1 {
    fn default() -> Self {
        Self {
            max_active_runs: 32,
            max_connections: 64,
            shutdown_grace_seconds: 30,
            readiness_cache_seconds: 30,
        }
    }
}

impl OperationalLimitsV1 {
    fn validate(self) -> Result<(), DeploymentConfigError> {
        if self.max_active_runs == 0
            || self.max_active_runs > 32
            || self.max_connections == 0
            || self.max_connections > 64
            || self.shutdown_grace_seconds == 0
            || self.shutdown_grace_seconds > 300
            || self.readiness_cache_seconds == 0
            || self.readiness_cache_seconds > 300
        {
            return Err(DeploymentConfigError::InvalidConfig);
        }
        Ok(())
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SecretReferenceV1 {
    pub file: PathBuf,
}

impl fmt::Debug for SecretReferenceV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SecretReferenceV1([REDACTED])")
    }
}

impl SecretReferenceV1 {
    fn validate(&self) -> Result<(), DeploymentConfigError> {
        require_absolute(&self.file)
    }

    pub fn resolve(&self) -> Result<ResolvedSecret, DeploymentConfigError> {
        self.validate()?;
        let metadata = std::fs::symlink_metadata(&self.file)
            .map_err(|_| DeploymentConfigError::SecretUnavailable)?;
        if !metadata.file_type().is_file()
            || metadata.uid() != rustix::process::geteuid().as_raw()
            || metadata.mode() & 0o077 != 0
            || metadata.len() == 0
            || metadata.len() > MAX_SECRET_BYTES as u64
        {
            return Err(DeploymentConfigError::SecretUnavailable);
        }
        let mut bytes =
            std::fs::read(&self.file).map_err(|_| DeploymentConfigError::SecretUnavailable)?;
        while bytes
            .last()
            .is_some_and(|byte| matches!(byte, b'\n' | b'\r'))
        {
            bytes.pop();
        }
        if bytes.is_empty() {
            return Err(DeploymentConfigError::SecretUnavailable);
        }
        Ok(ResolvedSecret(bytes))
    }
}

pub struct ResolvedSecret(Vec<u8>);

impl ResolvedSecret {
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.0
    }
}

impl fmt::Debug for ResolvedSecret {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ResolvedSecret([REDACTED])")
    }
}

impl Drop for ResolvedSecret {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct DeploymentFingerprint([u8; 32]);

impl DeploymentFingerprint {
    #[must_use]
    pub fn to_hex(self) -> String {
        hex(&self.0)
    }
}

impl fmt::Debug for DeploymentFingerprint {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("DeploymentFingerprint([REDACTED])")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProcessLifecycleV1 {
    Starting,
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
pub struct DependencyReadinessV1 {
    pub dependency: String,
    pub status: ReadinessStatusV1,
    pub code: String,
    pub checked_unix_seconds: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowReadinessV1 {
    pub workflow: String,
    pub enabled: bool,
    pub required: bool,
    pub status: ReadinessStatusV1,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReadinessSnapshotV1 {
    pub version: u16,
    pub overall: ReadinessStatusV1,
    pub dependencies: Vec<DependencyReadinessV1>,
    pub workflows: Vec<WorkflowReadinessV1>,
}

#[derive(Clone)]
pub struct ReadinessCache {
    inner: Arc<Mutex<ReadinessSnapshotV1>>,
}

impl ReadinessCache {
    #[must_use]
    pub fn new(snapshot: ReadinessSnapshotV1) -> Self {
        Self {
            inner: Arc::new(Mutex::new(snapshot)),
        }
    }

    pub fn snapshot(&self) -> Result<ReadinessSnapshotV1, DeploymentConfigError> {
        self.inner
            .lock()
            .map(|value| value.clone())
            .map_err(|_| DeploymentConfigError::OperationalState)
    }

    pub fn replace(&self, snapshot: ReadinessSnapshotV1) -> Result<(), DeploymentConfigError> {
        *self
            .inner
            .lock()
            .map_err(|_| DeploymentConfigError::OperationalState)? = snapshot;
        Ok(())
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OperationalMetricsSnapshotV1 {
    pub version: u16,
    pub epoch_unix_seconds: u64,
    pub runs_started: u64,
    pub runs_completed: u64,
    pub runs_failed: u64,
    pub runs_waiting: u64,
    pub run_duration_buckets: [u64; 5],
    pub approval_decisions: u64,
    pub approval_wait_latency_buckets: [u64; 5],
    pub recovered_completed: u64,
    pub recovered_failed: u64,
    pub recovered_waiting: u64,
    pub recovered_resumable: u64,
    pub recovered_manual_reconciliation: u64,
    pub model_latency_buckets: [u64; 5],
    pub retrieval_latency_buckets: [u64; 5],
    pub tool_latency_buckets: [u64; 5],
    pub active_runs: u64,
    pub active_connections: u64,
}

#[derive(Clone)]
pub struct OperationalMetrics {
    inner: Arc<Mutex<OperationalMetricsSnapshotV1>>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BuildInfoV1 {
    pub application: String,
    pub version: String,
    pub git_identity: Option<String>,
    pub deployment_fingerprint: String,
    pub config_schema_version: u16,
    pub store_schema_version: u16,
    pub event_schema_version: u16,
    pub checkpoint_schema_version: u16,
}

#[derive(Clone)]
pub struct OperationsState {
    readiness: ReadinessCache,
    metrics: OperationalMetrics,
    build: BuildInfoV1,
}

impl OperationsState {
    #[must_use]
    pub fn new(readiness: ReadinessCache, metrics: OperationalMetrics, build: BuildInfoV1) -> Self {
        Self {
            readiness,
            metrics,
            build,
        }
    }

    #[must_use]
    pub fn readiness_cache(&self) -> &ReadinessCache {
        &self.readiness
    }

    #[must_use]
    pub fn metrics(&self) -> &OperationalMetrics {
        &self.metrics
    }

    #[must_use]
    pub fn build(&self) -> &BuildInfoV1 {
        &self.build
    }
}

impl OperationalMetrics {
    #[must_use]
    pub fn new() -> Self {
        let epoch_unix_seconds = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or(Duration::ZERO)
            .as_secs();
        Self {
            inner: Arc::new(Mutex::new(OperationalMetricsSnapshotV1 {
                version: 1,
                epoch_unix_seconds,
                ..OperationalMetricsSnapshotV1::default()
            })),
        }
    }

    pub fn snapshot(&self) -> Result<OperationalMetricsSnapshotV1, DeploymentConfigError> {
        self.inner
            .lock()
            .map(|value| value.clone())
            .map_err(|_| DeploymentConfigError::OperationalState)
    }

    pub fn record_latency(
        &self,
        kind: MetricLatencyKind,
        duration: Duration,
    ) -> Result<(), DeploymentConfigError> {
        let index = latency_bucket(duration);
        let mut metrics = self
            .inner
            .lock()
            .map_err(|_| DeploymentConfigError::OperationalState)?;
        match kind {
            MetricLatencyKind::Model => metrics.model_latency_buckets[index] += 1,
            MetricLatencyKind::Retrieval => metrics.retrieval_latency_buckets[index] += 1,
            MetricLatencyKind::Tool => metrics.tool_latency_buckets[index] += 1,
        }
        Ok(())
    }

    pub fn run_started(&self) -> Result<(), DeploymentConfigError> {
        let mut metrics = self
            .inner
            .lock()
            .map_err(|_| DeploymentConfigError::OperationalState)?;
        metrics.runs_started = metrics.runs_started.saturating_add(1);
        metrics.active_runs = metrics.active_runs.saturating_add(1);
        Ok(())
    }

    pub fn run_settled(
        &self,
        disposition: Option<agent_service::RunDispositionV1>,
    ) -> Result<(), DeploymentConfigError> {
        let mut metrics = self
            .inner
            .lock()
            .map_err(|_| DeploymentConfigError::OperationalState)?;
        metrics.active_runs = metrics.active_runs.saturating_sub(1);
        match disposition {
            Some(agent_service::RunDispositionV1::Completed) => {
                metrics.runs_completed = metrics.runs_completed.saturating_add(1);
            }
            Some(agent_service::RunDispositionV1::Waiting) => {
                metrics.runs_waiting = metrics.runs_waiting.saturating_add(1);
            }
            Some(agent_service::RunDispositionV1::Failed)
            | Some(agent_service::RunDispositionV1::ManualReconciliationRequired)
            | None => {
                metrics.runs_failed = metrics.runs_failed.saturating_add(1);
            }
            Some(agent_service::RunDispositionV1::Resumable)
            | Some(agent_service::RunDispositionV1::Starting)
            | Some(agent_service::RunDispositionV1::Running) => {}
        }
        Ok(())
    }

    pub fn record_run_duration(&self, duration: Duration) -> Result<(), DeploymentConfigError> {
        let index = latency_bucket(duration);
        let mut metrics = self
            .inner
            .lock()
            .map_err(|_| DeploymentConfigError::OperationalState)?;
        metrics.run_duration_buckets[index] = metrics.run_duration_buckets[index].saturating_add(1);
        Ok(())
    }

    pub fn record_approval_wait(&self, duration: Duration) -> Result<(), DeploymentConfigError> {
        let index = latency_bucket(duration);
        let mut metrics = self
            .inner
            .lock()
            .map_err(|_| DeploymentConfigError::OperationalState)?;
        metrics.approval_decisions = metrics.approval_decisions.saturating_add(1);
        metrics.approval_wait_latency_buckets[index] =
            metrics.approval_wait_latency_buckets[index].saturating_add(1);
        Ok(())
    }

    pub fn record_recovery(
        &self,
        disposition: agent_service::RunDispositionV1,
    ) -> Result<(), DeploymentConfigError> {
        let mut metrics = self
            .inner
            .lock()
            .map_err(|_| DeploymentConfigError::OperationalState)?;
        match disposition {
            agent_service::RunDispositionV1::Completed => {
                metrics.recovered_completed = metrics.recovered_completed.saturating_add(1);
            }
            agent_service::RunDispositionV1::Failed => {
                metrics.recovered_failed = metrics.recovered_failed.saturating_add(1);
            }
            agent_service::RunDispositionV1::Waiting => {
                metrics.recovered_waiting = metrics.recovered_waiting.saturating_add(1);
                metrics.runs_waiting = metrics.runs_waiting.saturating_add(1);
            }
            agent_service::RunDispositionV1::Resumable => {
                metrics.recovered_resumable = metrics.recovered_resumable.saturating_add(1);
            }
            agent_service::RunDispositionV1::ManualReconciliationRequired => {
                metrics.recovered_manual_reconciliation =
                    metrics.recovered_manual_reconciliation.saturating_add(1);
            }
            agent_service::RunDispositionV1::Starting
            | agent_service::RunDispositionV1::Running => {}
        }
        Ok(())
    }

    pub fn connection_opened(&self) -> Result<(), DeploymentConfigError> {
        let mut metrics = self
            .inner
            .lock()
            .map_err(|_| DeploymentConfigError::OperationalState)?;
        metrics.active_connections = metrics.active_connections.saturating_add(1);
        Ok(())
    }

    pub fn connection_closed(&self) -> Result<(), DeploymentConfigError> {
        let mut metrics = self
            .inner
            .lock()
            .map_err(|_| DeploymentConfigError::OperationalState)?;
        metrics.active_connections = metrics.active_connections.saturating_sub(1);
        Ok(())
    }
}

impl Default for OperationalMetrics {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MetricLatencyKind {
    Model,
    Retrieval,
    Tool,
}

impl ServiceObserver for OperationalMetrics {
    fn run_started(&self) {
        let _ = self.run_started();
    }

    fn run_settled(
        &self,
        disposition: Option<agent_service::RunDispositionV1>,
        duration: Duration,
    ) {
        let _ = self.run_settled(disposition);
        let _ = self.record_run_duration(duration);
    }
}

fn latency_bucket(duration: Duration) -> usize {
    match duration.as_millis() {
        0..=99 => 0,
        100..=499 => 1,
        500..=1_999 => 2,
        2_000..=9_999 => 3,
        _ => 4,
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompatibilityManifestV1 {
    pub version: u16,
    pub build_identity: String,
    pub git_identity: Option<String>,
    pub store_schema_version: u16,
    pub event_schema_version: u16,
    pub checkpoint_schema_version: u16,
    pub workflow_contracts: Vec<String>,
    pub graph_digests: Vec<String>,
    pub capsule_version: u16,
    pub tool_contract_digests: Vec<String>,
    pub seal_key_ids: Vec<String>,
    pub workspace_bindings: Vec<WorkspaceBindingId>,
    pub mcp_fingerprints: Vec<String>,
    pub containment_protocol_version: u16,
    pub containment_artifacts: Vec<String>,
}

impl CompatibilityManifestV1 {
    pub fn compatible_with(&self, other: &Self) -> bool {
        self.store_schema_version == other.store_schema_version
            && self.event_schema_version == other.event_schema_version
            && self.checkpoint_schema_version == other.checkpoint_schema_version
            && self.workflow_contracts == other.workflow_contracts
            && self.graph_digests == other.graph_digests
            && self.capsule_version == other.capsule_version
            && self.tool_contract_digests == other.tool_contract_digests
            && self.seal_key_ids == other.seal_key_ids
            && self.workspace_bindings == other.workspace_bindings
            && self.mcp_fingerprints == other.mcp_fingerprints
            && self.containment_protocol_version == other.containment_protocol_version
            && self.containment_artifacts == other.containment_artifacts
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackupManifestV1 {
    pub version: u16,
    pub created_unix_seconds: u64,
    pub deployment_fingerprint: String,
    pub database_file: String,
    pub database_sha256: String,
    pub audit_file: String,
    pub audit_sha256: String,
    pub compatibility: CompatibilityManifestV1,
}

pub struct DeploymentLock {
    _file: File,
}

impl DeploymentLock {
    pub fn acquire(data_directory: &Path) -> Result<Self, DeploymentConfigError> {
        require_absolute(data_directory)?;
        std::fs::create_dir_all(data_directory)
            .map_err(|_| DeploymentConfigError::ConfigurationUnavailable)?;
        let metadata = std::fs::symlink_metadata(data_directory)
            .map_err(|_| DeploymentConfigError::ConfigurationUnavailable)?;
        if !metadata.file_type().is_dir() || metadata.uid() != rustix::process::geteuid().as_raw() {
            return Err(DeploymentConfigError::ConfigurationUnavailable);
        }
        std::fs::set_permissions(data_directory, Permissions::from_mode(0o700))
            .map_err(|_| DeploymentConfigError::ConfigurationUnavailable)?;
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(data_directory.join("service.lock"))
            .map_err(|_| DeploymentConfigError::ConfigurationUnavailable)?;
        file.set_permissions(Permissions::from_mode(0o600))
            .map_err(|_| DeploymentConfigError::ConfigurationUnavailable)?;
        flock(&file, FlockOperation::NonBlockingLockExclusive)
            .map_err(|_| DeploymentConfigError::LockUnavailable)?;
        Ok(Self { _file: file })
    }
}

impl CompatibilityManifestV1 {
    pub fn from_config(
        config: &DeploymentConfigV2,
        build_identity: impl Into<String>,
        git_identity: Option<String>,
    ) -> Result<Self, DeploymentConfigError> {
        config.validate()?;
        let route = knowledge_route(&config.knowledge)?;
        let mut workflow_contracts = Vec::new();
        let mut graph_digests = Vec::new();
        workflow_contracts.push(format!(
            "service-health:{}",
            serde_json::to_string(&RecoveryContract::NonRestartable)
                .map_err(|_| DeploymentConfigError::InvalidConfig)?
        ));
        for workflow in [
            Some(EnterpriseMvpWorkflow::readonly(route.clone())),
            config
                .workflows
                .local_write_enabled
                .then(|| EnterpriseMvpWorkflow::localwrite(route.clone())),
        ]
        .into_iter()
        .flatten()
        {
            let workflow = workflow.map_err(|_| DeploymentConfigError::InvalidConfig)?;
            let contract = workflow.recovery_contract();
            workflow_contracts.push(format!(
                "{}:{}",
                workflow.id().as_str(),
                serde_json::to_string(&contract)
                    .map_err(|_| DeploymentConfigError::InvalidConfig)?
            ));
            if let RecoveryContract::Graph {
                definition_digest, ..
            } = contract
            {
                graph_digests.push(hex(&definition_digest));
            }
        }
        let mut mcp_fingerprints = config
            .mcp
            .iter()
            .filter(|server| server.enabled)
            .map(|server| server.definition_fingerprint.to_ascii_lowercase())
            .collect::<Vec<_>>();
        mcp_fingerprints.sort();
        let (tool_contract_digests, seal_key_ids, workspace_bindings, containment_artifacts) =
            config.local_write.as_ref().map_or_else(
                || (Vec::new(), Vec::new(), Vec::new(), Vec::new()),
                |local| {
                    (
                        vec![local.tool_contract_sha256.to_ascii_lowercase()],
                        vec![local.seal_key_id.clone()],
                        vec![local.workspace_binding_id],
                        vec![
                            local.bwrap_sha256.to_ascii_lowercase(),
                            local.worker_sha256.to_ascii_lowercase(),
                        ],
                    )
                },
            );
        Ok(Self {
            version: 1,
            build_identity: build_identity.into(),
            git_identity,
            store_schema_version: agent_harness::CURRENT_STORE_SCHEMA_VERSION,
            event_schema_version: agent_core::CURRENT_EVENT_SCHEMA_VERSION.get(),
            checkpoint_schema_version: agent_harness::CURRENT_CHECKPOINT_SCHEMA_VERSION,
            workflow_contracts,
            graph_digests,
            capsule_version: agent_harness::LOCAL_WRITE_CAPSULE_VERSION,
            tool_contract_digests,
            seal_key_ids,
            workspace_bindings,
            mcp_fingerprints,
            containment_protocol_version: agent_containment_linux::LOCAL_WRITE_PROTOCOL_VERSION,
            containment_artifacts,
        })
    }
}

pub async fn create_backup(
    config: &DeploymentConfigV2,
    destination: &Path,
    compatibility: CompatibilityManifestV1,
) -> Result<BackupManifestV1, DeploymentConfigError> {
    config.validate()?;
    if !destination.is_absolute() || destination.exists() {
        return Err(DeploymentConfigError::BackupInvalid);
    }
    let _lock = DeploymentLock::acquire(&config.storage.data_directory)?;
    std::fs::create_dir(destination).map_err(|_| DeploymentConfigError::BackupInvalid)?;
    std::fs::set_permissions(destination, Permissions::from_mode(0o700))
        .map_err(|_| DeploymentConfigError::BackupInvalid)?;
    let database = destination.join("store.sqlite3");
    let audit = destination.join("audit.jsonl");
    let result = async {
        let admin = SqliteStoreAdmin::new(config.storage.database_path())
            .map_err(|_| DeploymentConfigError::BackupInvalid)?;
        let inspection = admin
            .backup_to(&database)
            .await
            .map_err(|_| DeploymentConfigError::BackupInvalid)?;
        validate_inspection(&inspection, &compatibility)?;
        copy_quiesced(&config.storage.audit_path(), &audit)?;
        let manifest = BackupManifestV1 {
            version: BACKUP_MANIFEST_VERSION,
            created_unix_seconds: unix_seconds(),
            deployment_fingerprint: config.fingerprint()?.to_hex(),
            database_file: "store.sqlite3".to_owned(),
            database_sha256: inspection.sha256,
            audit_file: "audit.jsonl".to_owned(),
            audit_sha256: file_sha256(&audit)?,
            compatibility,
        };
        write_manifest(destination, &manifest)?;
        Ok(manifest)
    }
    .await;
    if result.is_err() {
        let _ = std::fs::remove_dir_all(destination);
    }
    result
}

pub async fn verify_backup(
    source: &Path,
    expected: &CompatibilityManifestV1,
) -> Result<BackupManifestV1, DeploymentConfigError> {
    let manifest = read_manifest(source)?;
    if manifest.version != BACKUP_MANIFEST_VERSION
        || !manifest.compatibility.compatible_with(expected)
        || require_file_name(&manifest.database_file).is_err()
        || require_file_name(&manifest.audit_file).is_err()
    {
        return Err(DeploymentConfigError::IncompatibleBackup);
    }
    let database = source.join(&manifest.database_file);
    let audit = source.join(&manifest.audit_file);
    let inspection = SqliteStoreAdmin::inspect_path(database)
        .await
        .map_err(|_| DeploymentConfigError::BackupInvalid)?;
    if inspection.sha256 != manifest.database_sha256
        || file_sha256(&audit)? != manifest.audit_sha256
    {
        return Err(DeploymentConfigError::BackupInvalid);
    }
    validate_inspection(&inspection, expected)?;
    Ok(manifest)
}

pub async fn restore_backup(
    config: &DeploymentConfigV2,
    source: &Path,
    expected: &CompatibilityManifestV1,
) -> Result<(), DeploymentConfigError> {
    config.validate()?;
    let _lock = DeploymentLock::acquire(&config.storage.data_directory)?;
    if config.storage.database_path().exists() || config.storage.audit_path().exists() {
        return Err(DeploymentConfigError::RestoreTargetNotEmpty);
    }
    let manifest = verify_backup(source, expected).await?;
    copy_quiesced(
        &source.join(manifest.database_file),
        &config.storage.database_path(),
    )?;
    if let Err(error) = copy_quiesced(
        &source.join(manifest.audit_file),
        &config.storage.audit_path(),
    ) {
        let _ = std::fs::remove_file(config.storage.database_path());
        return Err(error);
    }
    let inspection = SqliteStoreAdmin::inspect_path(config.storage.database_path())
        .await
        .map_err(|_| DeploymentConfigError::BackupInvalid)?;
    if validate_inspection(&inspection, expected).is_err() {
        let _ = std::fs::remove_file(config.storage.database_path());
        let _ = std::fs::remove_file(config.storage.audit_path());
        return Err(DeploymentConfigError::IncompatibleBackup);
    }
    Ok(())
}

#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum DeploymentConfigError {
    #[error("deployment configuration is invalid")]
    InvalidConfig,
    #[error("deployment configuration is unavailable")]
    ConfigurationUnavailable,
    #[error("deployment secret is unavailable")]
    SecretUnavailable,
    #[error("deployment operational state is unavailable")]
    OperationalState,
    #[error("deployment data directory is already owned")]
    LockUnavailable,
    #[error("backup is invalid or corrupt")]
    BackupInvalid,
    #[error("backup contracts are incompatible")]
    IncompatibleBackup,
    #[error("restore target is not empty")]
    RestoreTargetNotEmpty,
}

fn knowledge_route(config: &KnowledgeConfigV1) -> Result<KnowledgeRoute, DeploymentConfigError> {
    match config {
        KnowledgeConfigV1::Ocpp { .. } => Ok(KnowledgeRoute::single(
            agent_core::KnowledgeBackendId::OcppRagKag,
        )),
        KnowledgeConfigV1::Standards { .. } => Ok(KnowledgeRoute::single(
            agent_core::KnowledgeBackendId::StandardsMcp,
        )),
        KnowledgeConfigV1::Federated { allow_partial, .. } => KnowledgeRoute::federated(
            vec![
                agent_core::KnowledgeBackendId::OcppRagKag,
                agent_core::KnowledgeBackendId::StandardsMcp,
            ],
            if *allow_partial {
                FederatedFailurePolicy::AllowPartial
            } else {
                FederatedFailurePolicy::RequireAll
            },
        )
        .map_err(|_| DeploymentConfigError::InvalidConfig),
    }
}

fn validate_inspection(
    inspection: &StoreInspection,
    expected: &CompatibilityManifestV1,
) -> Result<(), DeploymentConfigError> {
    let supported_contracts = expected
        .workflow_contracts
        .iter()
        .filter_map(|value| value.split_once(':').map(|(_, contract)| contract))
        .collect::<Vec<_>>();
    if inspection.store_schema_version != expected.store_schema_version
        || inspection
            .event_schema_versions
            .iter()
            .any(|value| *value != expected.event_schema_version)
        || inspection
            .checkpoint_schema_versions
            .iter()
            .any(|value| *value != expected.checkpoint_schema_version)
        || inspection
            .recovery_contracts
            .iter()
            .any(|value| !supported_contracts.contains(&value.as_str()))
        || inspection
            .capsule_versions
            .iter()
            .any(|value| *value != expected.capsule_version)
        || inspection
            .seal_key_ids
            .iter()
            .any(|value| !expected.seal_key_ids.contains(value))
        || inspection.workspace_bindings.iter().any(|value| {
            !expected
                .workspace_bindings
                .iter()
                .any(|expected| expected.to_string() == *value)
        })
        || inspection
            .tool_contract_digests
            .iter()
            .any(|value| !expected.tool_contract_digests.contains(value))
    {
        return Err(DeploymentConfigError::IncompatibleBackup);
    }
    Ok(())
}

fn copy_quiesced(source: &Path, destination: &Path) -> Result<(), DeploymentConfigError> {
    let trusted_source =
        std::fs::symlink_metadata(source).is_ok_and(|metadata| metadata.file_type().is_file());
    if destination.exists() || !trusted_source {
        return Err(DeploymentConfigError::BackupInvalid);
    }
    let mut input = File::open(source).map_err(|_| DeploymentConfigError::BackupInvalid)?;
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)
        .map_err(|_| DeploymentConfigError::BackupInvalid)?;
    output
        .set_permissions(Permissions::from_mode(0o600))
        .map_err(|_| DeploymentConfigError::BackupInvalid)?;
    std::io::copy(&mut input, &mut output).map_err(|_| DeploymentConfigError::BackupInvalid)?;
    output
        .sync_all()
        .map_err(|_| DeploymentConfigError::BackupInvalid)
}

fn write_manifest(
    directory: &Path,
    manifest: &BackupManifestV1,
) -> Result<(), DeploymentConfigError> {
    let path = directory.join("manifest.json");
    let bytes =
        serde_json::to_vec_pretty(manifest).map_err(|_| DeploymentConfigError::BackupInvalid)?;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|_| DeploymentConfigError::BackupInvalid)?;
    file.set_permissions(Permissions::from_mode(0o600))
        .map_err(|_| DeploymentConfigError::BackupInvalid)?;
    file.write_all(&bytes)
        .map_err(|_| DeploymentConfigError::BackupInvalid)?;
    file.sync_all()
        .map_err(|_| DeploymentConfigError::BackupInvalid)?;
    File::open(directory)
        .and_then(|directory| directory.sync_all())
        .map_err(|_| DeploymentConfigError::BackupInvalid)
}

fn read_manifest(directory: &Path) -> Result<BackupManifestV1, DeploymentConfigError> {
    if !directory.is_absolute() || !directory.is_dir() {
        return Err(DeploymentConfigError::BackupInvalid);
    }
    let bytes = std::fs::read(directory.join("manifest.json"))
        .map_err(|_| DeploymentConfigError::BackupInvalid)?;
    if bytes.len() > MAX_DEPLOYMENT_CONFIG_BYTES {
        return Err(DeploymentConfigError::BackupInvalid);
    }
    serde_json::from_slice(&bytes).map_err(|_| DeploymentConfigError::BackupInvalid)
}

fn file_sha256(path: &Path) -> Result<String, DeploymentConfigError> {
    let mut file = File::open(path).map_err(|_| DeploymentConfigError::BackupInvalid)?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|_| DeploymentConfigError::BackupInvalid)?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok(hex(&digest.finalize()))
}

fn unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_secs()
}

fn validate_process(
    executable: &Path,
    arguments: &[String],
    environment: &[EnvironmentEntryV1],
) -> Result<(), DeploymentConfigError> {
    require_absolute(executable)?;
    if arguments.len() > MAX_LIST_ITEMS || environment.len() > MAX_LIST_ITEMS {
        return Err(DeploymentConfigError::InvalidConfig);
    }
    for argument in arguments {
        bounded_text(argument)?;
    }
    for entry in environment {
        entry.validate()?;
    }
    Ok(())
}

fn validate_http_url(value: &str) -> Result<(), DeploymentConfigError> {
    let url = Url::parse(value).map_err(|_| DeploymentConfigError::InvalidConfig)?;
    matches!(url.scheme(), "http" | "https")
        .then_some(())
        .ok_or(DeploymentConfigError::InvalidConfig)
}

fn require_absolute(path: &Path) -> Result<(), DeploymentConfigError> {
    path.is_absolute()
        .then_some(())
        .ok_or(DeploymentConfigError::InvalidConfig)
}

fn require_file_name(value: &str) -> Result<(), DeploymentConfigError> {
    let path = Path::new(value);
    (!value.is_empty()
        && value.len() <= 128
        && path.file_name().is_some()
        && path.components().count() == 1)
        .then_some(())
        .ok_or(DeploymentConfigError::InvalidConfig)
}

fn require_sha256(value: &str) -> Result<(), DeploymentConfigError> {
    (value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .then_some(())
        .ok_or(DeploymentConfigError::InvalidConfig)
}

fn bounded_text(value: &str) -> Result<(), DeploymentConfigError> {
    (!value.is_empty() && value.len() <= MAX_TEXT_BYTES && !value.chars().any(char::is_control))
        .then_some(())
        .ok_or(DeploymentConfigError::InvalidConfig)
}

fn bounded_duration(value: u64) -> Result<(), DeploymentConfigError> {
    (value > 0 && value <= 120_000)
        .then_some(())
        .ok_or(DeploymentConfigError::InvalidConfig)
}

fn valid_nonempty_list(values: &[String]) -> bool {
    !values.is_empty()
        && values.len() <= MAX_LIST_ITEMS
        && values.iter().all(|value| bounded_text(value).is_ok())
}

fn valid_environment_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value.bytes().enumerate().all(|(index, byte)| {
            byte == b'_' || byte.is_ascii_alphabetic() || (index > 0 && byte.is_ascii_digit())
        })
}

fn default_model_probe_timeout_ms() -> u64 {
    10_000
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut value = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        value.push(char::from(DIGITS[usize::from(byte >> 4)]));
        value.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
    }
    value
}

pub fn restrict_file(path: &Path) -> Result<(), DeploymentConfigError> {
    std::fs::set_permissions(path, Permissions::from_mode(0o600))
        .map_err(|_| DeploymentConfigError::ConfigurationUnavailable)
}

#[cfg(test)]
mod tests;
