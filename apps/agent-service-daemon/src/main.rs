use std::{
    collections::HashMap,
    env,
    fs::{File, OpenOptions, Permissions},
    io::Write,
    os::unix::fs::{FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use agent_action_seal_local::LocalActionSealer;
use agent_containment_linux::{LinuxContainmentConfig, LinuxWorkspaceWriteTool};
use agent_core::{
    AgentEvent, AgentEventKind, ApprovalRequestId, ModelRequest, ModelResponse, RunBudget,
    ToolCall, ToolDefinition, ToolInput, ToolResult, WorkspaceBindingId,
};
use agent_deployment::{
    BuildInfoV1, DependencyReadinessV1, DeploymentConfigV1, DeploymentLock, EnvironmentEntryV1,
    KnowledgeConfigV1, ListenerConfigV1, LocalWriteConfigV1, MetricLatencyKind, ModelAuthConfigV1,
    ModelConfigV1, OperationalMetrics, OperationsState, ReadinessCache, ReadinessSnapshotV1,
    ReadinessStatusV1, WorkflowReadinessV1,
};
use agent_harness::{
    ApprovalPreview, AuditFailurePolicy, AuditPortError, AuditSink, ContainedInvocation,
    ContainedToolPort, ContainmentPortError, ExecutionHarness, HarnessConfig, M6ApprovalPolicy,
    ModelPort, ModelPortError, PortFuture, RecoveredRun, RecoveredWaitingRun, RecoveryContract,
    RunContext, RunKey, ToolRegistry, compute_tool_contract_digest,
};
use agent_knowledge::{
    EvidenceSet, FederatedFailurePolicy, KnowledgeBackendPort, KnowledgeError, KnowledgeFuture,
    KnowledgePort, KnowledgeQuery, KnowledgeRequest, KnowledgeRoute, RetrievalLimits,
    RoutedKnowledgePort,
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
use agent_service_http::{HttpSecurity, router_with_operations};
use anyhow::{Context, bail};
use sha2::{Digest, Sha256};

const DEPLOYMENT_CONFIG_ENV: &str = "ELA_DEPLOYMENT_CONFIG";

struct MeteredModel {
    inner: Arc<dyn ModelPort>,
    metrics: OperationalMetrics,
}

impl ModelPort for MeteredModel {
    fn invoke<'a>(
        &'a self,
        request: ModelRequest,
    ) -> PortFuture<'a, Result<ModelResponse, ModelPortError>> {
        Box::pin(async move {
            let started = std::time::Instant::now();
            let result = self.inner.invoke(request).await;
            let _ = self
                .metrics
                .record_latency(MetricLatencyKind::Model, started.elapsed());
            result
        })
    }
}

struct MeteredKnowledge {
    inner: Arc<dyn KnowledgePort>,
    metrics: OperationalMetrics,
}

impl KnowledgePort for MeteredKnowledge {
    fn retrieve<'a>(
        &'a self,
        request: KnowledgeRequest,
    ) -> KnowledgeFuture<'a, Result<EvidenceSet, KnowledgeError>> {
        Box::pin(async move {
            let started = std::time::Instant::now();
            let result = self.inner.retrieve(request).await;
            let _ = self
                .metrics
                .record_latency(MetricLatencyKind::Retrieval, started.elapsed());
            result
        })
    }
}

struct MeteredContainedTool {
    inner: Arc<dyn ContainedToolPort>,
    metrics: OperationalMetrics,
}

impl ContainedToolPort for MeteredContainedTool {
    fn definition(&self) -> &ToolDefinition {
        self.inner.definition()
    }

    fn approval_preview(&self, input: &ToolInput) -> Result<ApprovalPreview, ContainmentPortError> {
        self.inner.approval_preview(input)
    }

    fn start_contained(
        &self,
        call: ToolCall,
    ) -> Result<Box<dyn ContainedInvocation>, ContainmentPortError> {
        Ok(Box::new(MeteredContainedInvocation {
            inner: self.inner.start_contained(call)?,
            metrics: self.metrics.clone(),
            started: std::time::Instant::now(),
            recorded: false,
        }))
    }
}

struct MeteredContainedInvocation {
    inner: Box<dyn ContainedInvocation>,
    metrics: OperationalMetrics,
    started: std::time::Instant,
    recorded: bool,
}

impl MeteredContainedInvocation {
    fn record(&mut self) {
        if !self.recorded {
            let _ = self
                .metrics
                .record_latency(MetricLatencyKind::Tool, self.started.elapsed());
            self.recorded = true;
        }
    }
}

impl ContainedInvocation for MeteredContainedInvocation {
    fn wait<'a>(&'a mut self) -> PortFuture<'a, Result<ToolResult, ContainmentPortError>> {
        Box::pin(async move {
            let result = self.inner.wait().await;
            self.record();
            result
        })
    }

    fn terminate_and_reap<'a>(&'a mut self) -> PortFuture<'a, Result<(), ContainmentPortError>> {
        Box::pin(async move {
            let result = self.inner.terminate_and_reap().await;
            self.record();
            result
        })
    }
}

struct MetadataAudit {
    file: Arc<std::sync::Mutex<File>>,
    approval_started: std::sync::Mutex<HashMap<ApprovalRequestId, std::time::Instant>>,
    metrics: OperationalMetrics,
}

impl MetadataAudit {
    fn open(path: &Path, metrics: OperationalMetrics) -> anyhow::Result<Self> {
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
            .open(path)
            .context("failed to open metadata audit")?;
        std::fs::set_permissions(path, Permissions::from_mode(0o600))
            .context("failed to restrict metadata audit")?;
        Ok(Self {
            file: Arc::new(std::sync::Mutex::new(file)),
            approval_started: std::sync::Mutex::new(HashMap::new()),
            metrics,
        })
    }

    fn observe(&self, kind: &AgentEventKind) {
        let mut started = match self.approval_started.lock() {
            Ok(started) => started,
            Err(_) => return,
        };
        match kind {
            AgentEventKind::ApprovalRequested {
                approval_request_id,
                ..
            }
            | AgentEventKind::DurableApprovalPrepared {
                approval_request_id,
                ..
            } => {
                if started.len() < 64 {
                    started
                        .entry(*approval_request_id)
                        .or_insert_with(std::time::Instant::now);
                }
            }
            AgentEventKind::ApprovalGranted {
                approval_request_id,
            }
            | AgentEventKind::ApprovalDenied {
                approval_request_id,
            }
            | AgentEventKind::ApprovalFailed {
                approval_request_id,
                ..
            }
            | AgentEventKind::DurableApprovalDecisionRecorded {
                approval_request_id,
                ..
            } => {
                if let Some(since) = started.remove(approval_request_id) {
                    let _ = self.metrics.record_approval_wait(since.elapsed());
                }
            }
            _ => {}
        }
    }

    fn flush(&self) -> anyhow::Result<()> {
        self.file
            .lock()
            .map_err(|_| anyhow::anyhow!("metadata audit lock unavailable"))?
            .sync_all()
            .context("failed to flush metadata audit")
    }
}

impl AuditSink for MetadataAudit {
    fn record<'a>(&'a self, event: &'a AgentEvent) -> PortFuture<'a, Result<(), AuditPortError>> {
        let file = self.file.clone();
        let kind = event.kind().clone();
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
            .map_err(|_| AuditPortError::RecordFailed)??;
            self.observe(&kind);
            Ok(())
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

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_target(false)
        .without_time()
        .try_init()
        .map_err(|_| anyhow::anyhow!("failed to initialize tracing"))?;
    let config_path = env::var_os(DEPLOYMENT_CONFIG_ENV)
        .map(PathBuf::from)
        .context("ELA_DEPLOYMENT_CONFIG is required")?;
    let config = DeploymentConfigV1::load(&config_path)?;
    let fingerprint = config.fingerprint()?.to_hex();
    let metrics = OperationalMetrics::new();
    let _data = DeploymentLock::acquire(&config.storage.data_directory)?;
    let persistence = Arc::new(SqliteRunPersistence::open(config.storage.database_path()).await?);
    let audit = Arc::new(MetadataAudit::open(
        &config.storage.audit_path(),
        metrics.clone(),
    )?);
    let model_config = model_config(&config.model)?;
    probe_openai_compatible(&model_config)
        .await
        .context("configured model readiness failed")?;
    let model = build_openai_compatible_model_port(model_config)
        .context("failed to construct configured model")?;
    let model: Arc<dyn ModelPort> = Arc::new(MeteredModel {
        inner: model,
        metrics: metrics.clone(),
    });
    let (knowledge, route) = knowledge_config(&config.knowledge)?;
    tokio::time::timeout(
        Duration::from_millis(config.model.readiness_timeout_ms),
        knowledge.retrieve(KnowledgeRequest::new(
            KnowledgeQuery::new(config.knowledge.readiness_query())?,
            route.clone(),
            RetrievalLimits::new(1, 512, 512)?,
        )),
    )
    .await
    .context("configured knowledge readiness timed out")?
    .context("configured knowledge readiness failed")?;
    let knowledge: Arc<dyn KnowledgePort> = Arc::new(MeteredKnowledge {
        inner: knowledge,
        metrics: metrics.clone(),
    });

    let mut tools = ToolRegistry::new();
    let localwrite = match config.local_write.as_ref() {
        Some(local_write) => match localwrite_config(local_write).await {
            Ok(value) => Some(value),
            Err(_) => {
                tracing::warn!("LocalWrite workflow readiness failed; workflow unavailable");
                None
            }
        },
        None => None,
    };
    let localwrite_enabled = localwrite.is_some();
    let localwrite_ready = if let Some((tool, sealer, workspace_binding)) = localwrite {
        let inner: Arc<dyn ContainedToolPort> = tool;
        let port: Arc<dyn ContainedToolPort> = Arc::new(MeteredContainedTool {
            inner,
            metrics: metrics.clone(),
        });
        tools.register_contained(port)?;
        Some((sealer, workspace_binding))
    } else {
        None
    };
    let mut harness = ExecutionHarness::new(
        model,
        tools,
        Arc::new(M6ApprovalPolicy),
        audit.clone(),
        HarnessConfig::new(AuditFailurePolicy::FailClosed, Duration::from_secs(10))?,
    )
    .with_persistence_port(persistence.clone())
    .with_knowledge_port(knowledge);
    if let Some((sealer, workspace_binding)) = localwrite_ready {
        harness = harness.with_durable_local_write_approval(sealer, workspace_binding);
    }
    let harness = Arc::new(harness);
    let workflows = workflow_catalog(route, localwrite_enabled)?;
    let service = Arc::new(
        AgentService::new(harness, persistence, workflows)?
            .with_observer(Arc::new(metrics.clone())),
    );
    let discovered = service.discover_runs().await?;
    for run in &discovered {
        let _ = metrics.record_recovery(run.disposition);
    }
    tracing::info!(
        durable_runs = discovered.len(),
        "durable run catalog loaded"
    );

    let checked_at = unix_seconds();
    let localwrite_status = if config.workflows.local_write_enabled {
        if localwrite_enabled {
            ReadinessStatusV1::Ready
        } else {
            ReadinessStatusV1::Unavailable
        }
    } else {
        ReadinessStatusV1::Unavailable
    };
    let overall = if config.workflows.local_write_required && !localwrite_enabled {
        ReadinessStatusV1::Degraded
    } else {
        ReadinessStatusV1::Ready
    };
    let mut dependencies = vec![
        ready_dependency("sqlite", checked_at),
        ready_dependency("audit", checked_at),
        ready_dependency("model", checked_at),
        ready_dependency("knowledge", checked_at),
        DependencyReadinessV1 {
            dependency: "containment".to_owned(),
            status: localwrite_status,
            code: if localwrite_enabled {
                "bounded_probe_passed"
            } else {
                "unavailable"
            }
            .to_owned(),
            checked_unix_seconds: checked_at,
        },
    ];
    if config.workflows.local_write_enabled {
        for dependency in ["action_seal_key", "workspace_binding", "tool_contract"] {
            dependencies.push(DependencyReadinessV1 {
                dependency: dependency.to_owned(),
                status: localwrite_status,
                code: if localwrite_enabled {
                    "verified"
                } else {
                    "unavailable"
                }
                .to_owned(),
                checked_unix_seconds: checked_at,
            });
        }
    }
    for server in config.mcp.iter().filter(|server| server.enabled) {
        dependencies.push(DependencyReadinessV1 {
            dependency: format!("mcp:{}", server.server_id),
            status: ReadinessStatusV1::Degraded,
            code: "configured_not_required_by_mvp".to_owned(),
            checked_unix_seconds: checked_at,
        });
    }
    let operations = OperationsState::new(
        ReadinessCache::new(ReadinessSnapshotV1 {
            version: 1,
            overall,
            dependencies,
            workflows: vec![
                WorkflowReadinessV1 {
                    workflow: agent_mvp::READONLY_WORKFLOW_ID.to_owned(),
                    enabled: true,
                    required: config.workflows.readonly_required,
                    status: ReadinessStatusV1::Ready,
                },
                WorkflowReadinessV1 {
                    workflow: agent_mvp::LOCALWRITE_WORKFLOW_ID.to_owned(),
                    enabled: config.workflows.local_write_enabled,
                    required: config.workflows.local_write_required,
                    status: localwrite_status,
                },
            ],
        }),
        metrics,
        BuildInfoV1 {
            application: "enterprise-local-agent".to_owned(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
            git_identity: option_env!("ELA_GIT_IDENTITY").map(str::to_owned),
            deployment_fingerprint: fingerprint,
            config_schema_version: agent_deployment::DEPLOYMENT_CONFIG_SCHEMA_VERSION,
            store_schema_version: agent_harness::CURRENT_STORE_SCHEMA_VERSION,
            event_schema_version: agent_core::CURRENT_EVENT_SCHEMA_VERSION.get(),
            checkpoint_schema_version: agent_harness::CURRENT_CHECKPOINT_SCHEMA_VERSION,
        },
    );
    let shutdown_grace = Duration::from_secs(config.limits.shutdown_grace_seconds);
    match &config.listener {
        ListenerConfigV1::LoopbackTcp {
            address,
            bearer,
            allowed_hosts,
            allowed_origins,
        } => {
            let address: std::net::SocketAddr =
                address.parse().context("invalid loopback address")?;
            let bearer = bearer.resolve()?;
            let bearer =
                std::str::from_utf8(bearer.bytes()).context("loopback bearer is not UTF-8")?;
            let listener = tokio::net::TcpListener::bind(address).await?;
            axum::serve(
                listener,
                router_with_operations(
                    service.clone(),
                    HttpSecurity::loopback(bearer, allowed_hosts.clone(), allowed_origins.clone())
                        .map_err(|_| anyhow::anyhow!("invalid loopback security configuration"))?,
                    operations,
                ),
            )
            .with_graceful_shutdown(shutdown_sequence(service, audit, shutdown_grace))
            .await
            .context("service failed")
        }
        ListenerConfigV1::Unix { socket_path } => {
            prepare_socket(socket_path)?;
            let listener = tokio::net::UnixListener::bind(socket_path)
                .context("failed to bind service socket")?;
            std::fs::set_permissions(socket_path, Permissions::from_mode(0o600))
                .context("failed to restrict service socket")?;
            let result = axum::serve(
                listener,
                router_with_operations(service.clone(), HttpSecurity::unix_socket(), operations),
            )
            .with_graceful_shutdown(shutdown_sequence(service, audit, shutdown_grace))
            .await;
            let _ = std::fs::remove_file(socket_path);
            result.context("service failed")
        }
    }
}

fn model_config(model: &ModelConfigV1) -> anyhow::Result<OpenAiCompatibleConfig> {
    let label = ProviderLabel::new(model.provider_label.clone())?;
    let config = match &model.auth {
        ModelAuthConfigV1::Bearer { credential } => {
            let credential = credential.resolve()?;
            let value = std::str::from_utf8(credential.bytes())
                .context("model bearer credential is not UTF-8")?;
            OpenAiCompatibleConfig::new(
                model.base_url.clone(),
                model.model_id.clone(),
                BearerCredential::new(value.to_owned())?,
            )?
        }
        ModelAuthConfigV1::NoAuthLoopback => OpenAiCompatibleConfig::new_no_auth_loopback(
            model.base_url.clone(),
            model.model_id.clone(),
        )?,
    };
    let config = config
        .with_provider_label(label)
        .with_json_object_output(256)?;
    Ok(if model.disable_reasoning {
        config.with_reasoning_disabled()
    } else {
        config
    })
}

fn knowledge_config(
    config: &KnowledgeConfigV1,
) -> anyhow::Result<(Arc<dyn KnowledgePort>, KnowledgeRoute)> {
    let mut backends: Vec<Arc<dyn KnowledgeBackendPort>> = Vec::new();
    let route = match config {
        KnowledgeConfigV1::Ocpp { base_url, .. } => {
            backends.push(ocpp_backend(base_url)?);
            KnowledgeRoute::single(agent_core::KnowledgeBackendId::OcppRagKag)
        }
        KnowledgeConfigV1::Standards {
            executable,
            arguments,
            environment,
            ..
        } => {
            backends.push(standards_backend(executable, arguments, environment)?);
            KnowledgeRoute::single(agent_core::KnowledgeBackendId::StandardsMcp)
        }
        KnowledgeConfigV1::Federated {
            ocpp_base_url,
            standards_executable,
            standards_arguments,
            standards_environment,
            allow_partial,
            ..
        } => {
            backends.push(ocpp_backend(ocpp_base_url)?);
            backends.push(standards_backend(
                standards_executable,
                standards_arguments,
                standards_environment,
            )?);
            KnowledgeRoute::federated(
                vec![
                    agent_core::KnowledgeBackendId::OcppRagKag,
                    agent_core::KnowledgeBackendId::StandardsMcp,
                ],
                if *allow_partial {
                    FederatedFailurePolicy::AllowPartial
                } else {
                    FederatedFailurePolicy::RequireAll
                },
            )?
        }
    };
    Ok((Arc::new(RoutedKnowledgePort::new(backends)?), route))
}

fn ocpp_backend(base_url: &str) -> anyhow::Result<Arc<dyn KnowledgeBackendPort>> {
    let url = url::Url::parse(base_url)?;
    let config = OcppApiConfig::new(url)
        .map_err(|_| anyhow::anyhow!("OCPP knowledge configuration is invalid"))?;
    let adapter = OcppKnowledgeAdapter::new(config)
        .map_err(|_| anyhow::anyhow!("OCPP knowledge adapter construction failed"))?;
    Ok(Arc::new(adapter))
}

fn standards_backend(
    executable: &Path,
    arguments: &[String],
    environment: &[EnvironmentEntryV1],
) -> anyhow::Result<Arc<dyn KnowledgeBackendPort>> {
    let environment = resolve_environment(environment)?;
    let config = StandardsMcpConfig::new(
        executable.to_path_buf(),
        arguments.iter().cloned().map(Into::into).collect(),
        environment
            .into_iter()
            .map(|(key, value)| (key.into(), value.into()))
            .collect(),
    )
    .map_err(|_| anyhow::anyhow!("standards MCP configuration is invalid"))?;
    Ok(Arc::new(StandardsMcpKnowledgeAdapter::new(config)))
}

async fn localwrite_config(
    config: &LocalWriteConfigV1,
) -> anyhow::Result<(
    Arc<LinuxWorkspaceWriteTool>,
    Arc<LocalActionSealer>,
    WorkspaceBindingId,
)> {
    verify_artifact(&config.bwrap_path, &config.bwrap_sha256)?;
    verify_artifact(&config.worker_path, &config.worker_sha256)?;
    let tool = Arc::new(
        LinuxWorkspaceWriteTool::probe_and_create(LinuxContainmentConfig::new(
            config.workspace.clone(),
            config.bwrap_path.clone(),
            config.worker_path.clone(),
        ))
        .await?,
    );
    let digest = compute_tool_contract_digest(agent_harness::ContainedToolPort::definition(&*tool));
    if bytes_to_hex(digest.as_bytes()) != config.tool_contract_sha256.to_ascii_lowercase() {
        bail!("LocalWrite tool contract mismatch");
    }
    let key = config.seal_key.resolve()?;
    let key = decode_key(std::str::from_utf8(key.bytes()).context("seal key is not UTF-8")?)?;
    let sealer = Arc::new(LocalActionSealer::new(config.seal_key_id.clone(), key)?);
    Ok((tool, sealer, config.workspace_binding_id))
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

fn resolve_environment(entries: &[EnvironmentEntryV1]) -> anyhow::Result<Vec<(String, String)>> {
    entries
        .iter()
        .map(|entry| {
            let value = if let Some(literal) = &entry.literal {
                literal.clone()
            } else {
                let secret = entry
                    .secret
                    .as_ref()
                    .context("environment value missing")?
                    .resolve()?;
                std::str::from_utf8(secret.bytes())
                    .context("environment secret is not UTF-8")?
                    .to_owned()
            };
            Ok((entry.name.clone(), value))
        })
        .collect()
}

fn verify_artifact(path: &Path, expected: &str) -> anyhow::Result<()> {
    let metadata = std::fs::symlink_metadata(path).context("artifact unavailable")?;
    if !metadata.file_type().is_file()
        || metadata.uid() != 0
        || metadata.mode() & 0o111 == 0
        || metadata.mode() & 0o222 != 0
        || metadata.mode() & (libc::S_ISUID | libc::S_ISGID) != 0
    {
        bail!("artifact trust validation failed");
    }
    let bytes = std::fs::read(path).context("artifact unavailable")?;
    let digest = Sha256::digest(bytes);
    if bytes_to_hex(digest.as_slice()) != expected.to_ascii_lowercase() {
        bail!("artifact identity mismatch");
    }
    Ok(())
}

fn bytes_to_hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(char::from(DIGITS[usize::from(byte >> 4)]));
        output.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
    }
    output
}

fn unix_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_secs()
}

fn ready_dependency(name: &str, checked_unix_seconds: u64) -> DependencyReadinessV1 {
    DependencyReadinessV1 {
        dependency: name.to_owned(),
        status: ReadinessStatusV1::Ready,
        code: "ready".to_owned(),
        checked_unix_seconds,
    }
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

async fn shutdown_sequence(service: Arc<AgentService>, audit: Arc<MetadataAudit>, grace: Duration) {
    wait_for_shutdown_signal().await;
    if service.drain(grace).await.is_err() {
        tracing::error!("service drain did not settle before the configured deadline");
    }
    if audit.flush().is_err() {
        tracing::error!("metadata audit flush failed during shutdown");
    }
}

async fn wait_for_shutdown_signal() {
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
        let first = DeploymentLock::acquire(&path).expect("first owner");
        assert!(DeploymentLock::acquire(&path).is_err());
        drop(first);
        DeploymentLock::acquire(&path).expect("lock released when owner exits");
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
