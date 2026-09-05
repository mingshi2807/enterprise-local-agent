#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

#[cfg(not(target_os = "linux"))]
compile_error!("agent-containment-linux supports Linux only");

mod fd_map;
mod launcher;
mod path;
mod protocol;

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::os::fd::{AsFd, OwnedFd};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use agent_core::{
    CapabilityKind, ToolCall, ToolDefinition, ToolDomainFailure, ToolDomainFailureKind, ToolName,
    ToolOutput, ToolResult, ToolSchema,
};
use agent_harness::{
    ApprovalPreview, ContainedInvocation, ContainedToolPort, ContainmentPortError, PortFuture,
};
use rustix::fs::{Mode, OFlags, open};
use serde_json::json;
use thiserror::Error;
use tokio::process::Command;

use crate::launcher::{SpawnedWorker, probe_lifecycle_support};
use crate::path::{InputError, parse_input, resolve_parent};
use crate::protocol::{
    LandlockState, NamespaceIds, PROTOCOL_VERSION, WorkerErrorCode, WorkerRequest, WorkerResponse,
    WorkerWriteResult,
};

const MINIMUM_BWRAP_VERSION: (u32, u32, u32) = (0, 11, 2);
const PROBE_TIMEOUT: Duration = Duration::from_secs(3);

pub const WORKSPACE_WRITE_FILE_TOOL_NAME: &str = "workspace_write_file";

pub struct LinuxContainmentConfig {
    workspace_root: PathBuf,
    bwrap_path: PathBuf,
    worker_path: PathBuf,
}

impl LinuxContainmentConfig {
    #[must_use]
    pub fn new(
        workspace_root: impl Into<PathBuf>,
        bwrap_path: impl Into<PathBuf>,
        worker_path: impl Into<PathBuf>,
    ) -> Self {
        Self {
            workspace_root: workspace_root.into(),
            bwrap_path: bwrap_path.into(),
            worker_path: worker_path.into(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LandlockCapability {
    FullyEnforced,
    PartiallyEnforced,
    Unavailable,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinuxContainmentCapabilities {
    bwrap_version: String,
    landlock: LandlockCapability,
}

impl LinuxContainmentCapabilities {
    #[must_use]
    pub fn bwrap_version(&self) -> &str {
        &self.bwrap_version
    }

    #[must_use]
    pub const fn landlock(&self) -> LandlockCapability {
        self.landlock
    }
}

#[derive(Clone, Copy, Debug, Error, PartialEq, Eq)]
pub enum LinuxContainmentUnavailable {
    #[error("bubblewrap is unavailable or unsupported")]
    Bubblewrap,
    #[error("the worker artifact is unavailable or not self-contained")]
    Worker,
    #[error("the trusted workspace is unavailable")]
    Workspace,
    #[error("the production containment probe failed")]
    Probe,
}

pub struct LinuxWorkspaceWriteTool {
    definition: ToolDefinition,
    bwrap_path: PathBuf,
    workspace: OwnedFd,
    worker: OwnedFd,
    capabilities: LinuxContainmentCapabilities,
}

impl LinuxWorkspaceWriteTool {
    pub async fn probe_and_create(
        config: LinuxContainmentConfig,
    ) -> Result<Self, LinuxContainmentUnavailable> {
        let bwrap_version = validate_bwrap(&config.bwrap_path).await?;
        let workspace = open_trusted_directory(&config.workspace_root)
            .map_err(|_| LinuxContainmentUnavailable::Workspace)?;
        let worker =
            open_worker(&config.worker_path).map_err(|_| LinuxContainmentUnavailable::Worker)?;
        verify_self_contained(&worker).map_err(|_| LinuxContainmentUnavailable::Worker)?;
        probe_lifecycle_support().map_err(|_| LinuxContainmentUnavailable::Probe)?;

        let definition =
            workspace_write_definition().map_err(|_| LinuxContainmentUnavailable::Worker)?;
        let landlock = run_probe(&config.bwrap_path, &workspace, &worker).await?;
        Ok(Self {
            definition,
            bwrap_path: config.bwrap_path,
            workspace,
            worker,
            capabilities: LinuxContainmentCapabilities {
                bwrap_version,
                landlock,
            },
        })
    }

    #[must_use]
    pub const fn capabilities(&self) -> &LinuxContainmentCapabilities {
        &self.capabilities
    }

    /// Runs the process-tree kill/reap certification hook.
    ///
    /// This API exists only in certification builds and is never reachable
    /// from a model-proposed action.
    #[cfg(feature = "certification-hooks")]
    pub async fn certify_process_reaping(&self) -> Result<(), LinuxContainmentUnavailable> {
        certify_no_xdev()?;
        const READY: &str = ".ela-m6-1-certification-ready";
        let request = WorkerRequest::CertificationHold {
            version: PROTOCOL_VERSION,
        };
        let mut process =
            SpawnedWorker::spawn(&self.bwrap_path, &self.workspace, &self.worker, &request)
                .map_err(|_| LinuxContainmentUnavailable::Probe)?;
        process
            .send_request()
            .await
            .map_err(|_| LinuxContainmentUnavailable::Probe)?;
        let bwrap_pid = process.pid().ok_or(LinuxContainmentUnavailable::Probe)?;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        let mut marker_seen = false;
        let descendants = loop {
            marker_seen |= certification_marker_exists(&self.workspace, READY);
            if let Some(descendants) = process_descendants(bwrap_pid)
                && marker_seen
                && descendants.len() >= 3
            {
                break descendants;
            }
            if tokio::time::Instant::now() >= deadline {
                let _ = process.terminate_and_reap().await;
                remove_certification_marker(&self.workspace, READY);
                return Err(LinuxContainmentUnavailable::Probe);
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        };
        let bwrap = process_identity(bwrap_pid).ok_or(LinuxContainmentUnavailable::Probe)?;
        process
            .terminate_and_reap()
            .await
            .map_err(|_| LinuxContainmentUnavailable::Probe)?;
        remove_certification_marker(&self.workspace, READY);
        let mut processes = descendants;
        processes.push(bwrap);
        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        while processes.iter().copied().any(process_identity_exists) {
            if tokio::time::Instant::now() >= deadline {
                return Err(LinuxContainmentUnavailable::Probe);
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        Ok(())
    }
}

#[cfg(feature = "certification-hooks")]
fn certify_no_xdev() -> Result<(), LinuxContainmentUnavailable> {
    let root =
        open_trusted_directory(Path::new("/")).map_err(|_| LinuxContainmentUnavailable::Probe)?;
    match resolve_parent(root.as_fd(), "proc") {
        Err(rustix::io::Errno::XDEV) => Ok(()),
        _ => Err(LinuxContainmentUnavailable::Probe),
    }
}

impl ContainedToolPort for LinuxWorkspaceWriteTool {
    fn definition(&self) -> &ToolDefinition {
        &self.definition
    }

    fn approval_preview(
        &self,
        input: &agent_core::ToolInput,
    ) -> Result<ApprovalPreview, ContainmentPortError> {
        let input =
            parse_input(input.as_value()).map_err(|_| ContainmentPortError::PreviewRejected)?;
        ApprovalPreview::new(
            format!("workspace_write_file ({} bytes)", input.content.len()),
            input.relative_path,
        )
        .map_err(ContainmentPortError::from)
    }

    fn start_contained(
        &self,
        call: ToolCall,
    ) -> Result<Box<dyn ContainedInvocation>, ContainmentPortError> {
        if call.name().as_str() != WORKSPACE_WRITE_FILE_TOOL_NAME {
            return Err(ContainmentPortError::Infrastructure);
        }
        let input = parse_input(call.input().as_value()).map_err(|error| match error {
            InputError::Invalid | InputError::InvalidPath | InputError::ContentTooLarge => {
                ContainmentPortError::Infrastructure
            }
        })?;
        let parent = match resolve_parent(self.workspace.as_fd(), &input.parent) {
            Ok(parent) => parent,
            Err(error) => {
                return Ok(Box::new(ImmediateInvocation::domain_failure(
                    call.id(),
                    map_parent_failure(error),
                )));
            }
        };
        let request = WorkerRequest::WorkspaceWriteFile {
            version: PROTOCOL_VERSION,
            tool_call_id: call.id(),
            basename: input.basename,
            content: input.content,
        };
        let worker = SpawnedWorker::spawn(&self.bwrap_path, &parent, &self.worker, &request)
            .map_err(|_| ContainmentPortError::Unavailable)?;
        Ok(Box::new(LinuxInvocation {
            worker,
            call_id: call.id(),
        }))
    }
}

struct LinuxInvocation {
    worker: SpawnedWorker,
    call_id: agent_core::ToolCallId,
}

struct ImmediateInvocation {
    result: Option<ToolResult>,
}

impl ImmediateInvocation {
    const fn domain_failure(call_id: agent_core::ToolCallId, kind: ToolDomainFailureKind) -> Self {
        Self {
            result: Some(ToolResult::DomainFailure {
                call_id,
                failure: ToolDomainFailure::new(kind),
            }),
        }
    }
}

impl ContainedInvocation for ImmediateInvocation {
    fn wait<'a>(&'a mut self) -> PortFuture<'a, Result<ToolResult, ContainmentPortError>> {
        Box::pin(std::future::ready(
            self.result
                .take()
                .ok_or(ContainmentPortError::Infrastructure),
        ))
    }

    fn terminate_and_reap<'a>(&'a mut self) -> PortFuture<'a, Result<(), ContainmentPortError>> {
        self.result.take();
        Box::pin(std::future::ready(Ok(())))
    }
}

impl ContainedInvocation for LinuxInvocation {
    fn wait<'a>(&'a mut self) -> PortFuture<'a, Result<ToolResult, ContainmentPortError>> {
        Box::pin(async move {
            let response = self
                .worker
                .exchange()
                .await
                .map_err(|_| ContainmentPortError::Infrastructure)?;
            response_to_tool_result(response, self.call_id)
        })
    }

    fn terminate_and_reap<'a>(&'a mut self) -> PortFuture<'a, Result<(), ContainmentPortError>> {
        Box::pin(async move {
            self.worker
                .terminate_and_reap()
                .await
                .map_err(|_| ContainmentPortError::Infrastructure)
        })
    }
}

fn response_to_tool_result(
    response: WorkerResponse,
    expected_call_id: agent_core::ToolCallId,
) -> Result<ToolResult, ContainmentPortError> {
    let WorkerResponse::WorkspaceWriteFile {
        version,
        tool_call_id,
        result,
    } = response
    else {
        return Err(ContainmentPortError::Infrastructure);
    };
    if version != PROTOCOL_VERSION || tool_call_id != expected_call_id {
        return Err(ContainmentPortError::Infrastructure);
    }
    Ok(match result {
        WorkerWriteResult::Written { bytes_written } => ToolResult::Succeeded {
            call_id: tool_call_id,
            output: ToolOutput::new(json!({"bytes_written": bytes_written})),
        },
        WorkerWriteResult::Failed { code } => ToolResult::DomainFailure {
            call_id: tool_call_id,
            failure: ToolDomainFailure::new(map_domain_failure(code)),
        },
    })
}

const fn map_domain_failure(code: WorkerErrorCode) -> ToolDomainFailureKind {
    match code {
        WorkerErrorCode::InvalidInput => ToolDomainFailureKind::InvalidInput,
        WorkerErrorCode::NotFound => ToolDomainFailureKind::NotFound,
        WorkerErrorCode::Conflict => ToolDomainFailureKind::Conflict,
        WorkerErrorCode::Rejected | WorkerErrorCode::IoFailure | WorkerErrorCode::Unsupported => {
            ToolDomainFailureKind::Rejected
        }
    }
}

fn map_parent_failure(error: rustix::io::Errno) -> ToolDomainFailureKind {
    match error {
        rustix::io::Errno::NOENT => ToolDomainFailureKind::NotFound,
        rustix::io::Errno::NOTDIR => ToolDomainFailureKind::Conflict,
        _ => ToolDomainFailureKind::Rejected,
    }
}

fn workspace_write_definition() -> Result<ToolDefinition, agent_core::ToolDefinitionError> {
    ToolDefinition::new(
        ToolName::new(WORKSPACE_WRITE_FILE_TOOL_NAME)
            .map_err(|_| agent_core::ToolDefinitionError::EmptyDescription)?,
        "write one bounded UTF-8 file beneath the configured workspace",
        CapabilityKind::LocalWrite,
        ToolSchema::new(json!({
            "type": "object",
            "properties": {
                "relative_path": {"type": "string", "maxLength": 240},
                "content": {"type": "string", "maxLength": 4096}
            },
            "required": ["relative_path", "content"],
            "additionalProperties": false
        }))
        .map_err(|_| agent_core::ToolDefinitionError::EmptyDescription)?,
    )
}

async fn validate_bwrap(path: &Path) -> Result<String, LinuxContainmentUnavailable> {
    if !path.is_absolute() {
        return Err(LinuxContainmentUnavailable::Bubblewrap);
    }
    let metadata = std::fs::metadata(path).map_err(|_| LinuxContainmentUnavailable::Bubblewrap)?;
    if !metadata.is_file()
        || metadata.permissions().mode() & 0o111 == 0
        || metadata.uid() != 0
        || metadata.permissions().mode() & 0o022 != 0
        || metadata.mode() & libc::S_ISUID != 0
    {
        return Err(LinuxContainmentUnavailable::Bubblewrap);
    }
    let output = Command::new(path)
        .arg("--version")
        .env_clear()
        .stdin(Stdio::null())
        .output()
        .await
        .map_err(|_| LinuxContainmentUnavailable::Bubblewrap)?;
    if !output.status.success() || output.stdout.len() > 128 {
        return Err(LinuxContainmentUnavailable::Bubblewrap);
    }
    let text = std::str::from_utf8(&output.stdout)
        .map_err(|_| LinuxContainmentUnavailable::Bubblewrap)?
        .trim();
    let version = text
        .split_whitespace()
        .last()
        .and_then(parse_version)
        .ok_or(LinuxContainmentUnavailable::Bubblewrap)?;
    if version < MINIMUM_BWRAP_VERSION {
        return Err(LinuxContainmentUnavailable::Bubblewrap);
    }
    Ok(text.to_owned())
}

fn parse_version(value: &str) -> Option<(u32, u32, u32)> {
    let mut parts = value.split('.');
    let version = (
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
    );
    (parts.next().is_none()).then_some(version)
}

fn open_trusted_directory(path: &Path) -> Result<OwnedFd, rustix::io::Errno> {
    open(
        path,
        OFlags::PATH | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
        Mode::empty(),
    )
}

fn open_worker(path: &Path) -> std::io::Result<OwnedFd> {
    if !path.is_absolute() {
        return Err(std::io::Error::other("worker path is not absolute"));
    }
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
        .open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file()
        || metadata.permissions().mode() & 0o111 == 0
        || metadata.permissions().mode() & 0o222 != 0
        || metadata.mode() & (libc::S_ISUID | libc::S_ISGID) != 0
    {
        return Err(std::io::Error::other("worker is not executable"));
    }
    Ok(file.into())
}

fn verify_self_contained(worker: &OwnedFd) -> std::io::Result<()> {
    let duplicate = rustix::io::dup(worker)?;
    let mut file = File::from(duplicate);
    let mut header = [0_u8; 64];
    file.read_exact(&mut header)?;
    if &header[..4] != b"\x7fELF" || header[5] != 1 {
        return Err(std::io::Error::other(
            "worker is not a supported ELF artifact",
        ));
    }
    let (program_offset, entry_size, entry_count) = match header[4] {
        2 => (
            u64::from_le_bytes(header[32..40].try_into().map_err(|_| elf_error())?),
            u16::from_le_bytes(header[54..56].try_into().map_err(|_| elf_error())?),
            u16::from_le_bytes(header[56..58].try_into().map_err(|_| elf_error())?),
        ),
        1 => (
            u32::from_le_bytes(header[28..32].try_into().map_err(|_| elf_error())?) as u64,
            u16::from_le_bytes(header[42..44].try_into().map_err(|_| elf_error())?),
            u16::from_le_bytes(header[44..46].try_into().map_err(|_| elf_error())?),
        ),
        _ => return Err(elf_error()),
    };
    if entry_size < 4 || entry_count > 128 {
        return Err(elf_error());
    }
    for index in 0..entry_count {
        file.seek(SeekFrom::Start(
            program_offset + u64::from(index) * u64::from(entry_size),
        ))?;
        let mut kind = [0_u8; 4];
        file.read_exact(&mut kind)?;
        if u32::from_le_bytes(kind) == 3 {
            return Err(std::io::Error::other("worker has a dynamic interpreter"));
        }
    }
    Ok(())
}

fn elf_error() -> std::io::Error {
    std::io::Error::other("worker ELF metadata is invalid")
}

async fn run_probe(
    bwrap_path: &Path,
    workspace: &OwnedFd,
    worker: &OwnedFd,
) -> Result<LandlockCapability, LinuxContainmentUnavailable> {
    let listener = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .map_err(|_| LinuxContainmentUnavailable::Probe)?;
    let port = listener
        .local_addr()
        .map_err(|_| LinuxContainmentUnavailable::Probe)?
        .port();
    let request = WorkerRequest::Probe {
        version: PROTOCOL_VERSION,
        host_namespaces: host_namespace_ids().map_err(|_| LinuxContainmentUnavailable::Probe)?,
        host_probe_port: port,
    };
    let mut process = SpawnedWorker::spawn(bwrap_path, workspace, worker, &request)
        .map_err(|_| LinuxContainmentUnavailable::Probe)?;
    let response = match tokio::time::timeout(PROBE_TIMEOUT, process.exchange()).await {
        Ok(Ok(response)) => response,
        _ => {
            let _ = process.terminate_and_reap().await;
            return Err(LinuxContainmentUnavailable::Probe);
        }
    };
    let WorkerResponse::Probe {
        version,
        namespaces_isolated,
        network_isolated,
        environment_isolated,
        descriptors_isolated,
        home_isolated,
        no_new_privileges,
        capabilities_dropped,
        openat2_enforced,
        landlock,
    } = response
    else {
        return Err(LinuxContainmentUnavailable::Probe);
    };
    if version != PROTOCOL_VERSION
        || !namespaces_isolated
        || !network_isolated
        || !environment_isolated
        || !descriptors_isolated
        || !home_isolated
        || !no_new_privileges
        || !capabilities_dropped
        || !openat2_enforced
    {
        return Err(LinuxContainmentUnavailable::Probe);
    }
    Ok(match landlock {
        LandlockState::FullyEnforced => LandlockCapability::FullyEnforced,
        LandlockState::PartiallyEnforced => LandlockCapability::PartiallyEnforced,
        LandlockState::Unavailable => LandlockCapability::Unavailable,
    })
}

fn host_namespace_ids() -> std::io::Result<NamespaceIds> {
    Ok(NamespaceIds {
        user: std::fs::metadata("/proc/self/ns/user")?.ino(),
        mount: std::fs::metadata("/proc/self/ns/mnt")?.ino(),
        pid: std::fs::metadata("/proc/self/ns/pid")?.ino(),
        network: std::fs::metadata("/proc/self/ns/net")?.ino(),
    })
}

#[cfg(feature = "certification-hooks")]
#[derive(Clone, Copy)]
struct ProcessIdentity {
    pid: u32,
    start_time: u64,
}

#[cfg(feature = "certification-hooks")]
fn process_descendants(root: u32) -> Option<Vec<ProcessIdentity>> {
    let mut pending = vec![root];
    let mut descendants = Vec::new();
    for _ in 0..32 {
        let Some(pid) = pending.pop() else {
            return Some(descendants);
        };
        let children = std::fs::read_to_string(format!("/proc/{pid}/task/{pid}/children")).ok()?;
        for child in children.split_whitespace() {
            let child = child.parse().ok()?;
            if descendants.iter().any(|identity| identity.pid == child) {
                continue;
            }
            descendants.push(process_identity(child)?);
            pending.push(child);
        }
    }
    None
}

#[cfg(feature = "certification-hooks")]
fn process_identity(pid: u32) -> Option<ProcessIdentity> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let fields = stat.rsplit_once(") ")?.1;
    let start_time = fields.split_whitespace().nth(19)?.parse().ok()?;
    Some(ProcessIdentity { pid, start_time })
}

#[cfg(feature = "certification-hooks")]
fn process_identity_exists(expected: ProcessIdentity) -> bool {
    process_identity(expected.pid).is_some_and(|current| current.start_time == expected.start_time)
}

#[cfg(feature = "certification-hooks")]
fn certification_marker_exists(workspace: &OwnedFd, name: &str) -> bool {
    rustix::fs::openat2(
        workspace.as_fd(),
        name,
        OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
        Mode::empty(),
        crate::path::resolve_flags(),
    )
    .is_ok()
}

#[cfg(feature = "certification-hooks")]
fn remove_certification_marker(workspace: &OwnedFd, name: &str) {
    let _ = rustix::fs::unlinkat(workspace.as_fd(), name, rustix::fs::AtFlags::empty());
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    #[test]
    fn version_floor_rejects_the_current_legacy_release() {
        assert!(parse_version("0.6.1").expect("version parses") < MINIMUM_BWRAP_VERSION);
        assert!(parse_version("0.11.2").expect("version parses") >= MINIMUM_BWRAP_VERSION);
    }

    #[test]
    fn tool_schema_and_preview_are_bounded_and_content_free() {
        let definition = workspace_write_definition().expect("definition must be valid");
        assert_eq!(definition.capability(), CapabilityKind::LocalWrite);
        let tool = Arc::new(TestPreviewTool { definition });
        let preview = tool
            .approval_preview(&agent_core::ToolInput::new(json!({
                "relative_path": "reports/result.txt",
                "content": "secret-body"
            })))
            .expect("preview must be valid");
        assert!(preview.summary().contains("11 bytes"));
        assert_eq!(preview.target_label(), "reports/result.txt");
        assert!(!preview.summary().contains("secret-body"));
        let port: Arc<dyn ContainedToolPort> = tool;
        let mut registry = agent_harness::ToolRegistry::new();
        registry
            .register_contained(port)
            .expect("M5 must accept the bounded production schema");
    }

    #[test]
    fn worker_trust_checks_reject_relative_writable_and_setid_artifacts() {
        let directory = tempfile::tempdir().expect("temporary directory must be created");
        let worker = directory.path().join("worker");
        std::fs::write(&worker, b"worker").expect("worker fixture must be written");

        std::fs::set_permissions(&worker, std::fs::Permissions::from_mode(0o555))
            .expect("worker mode must be set");
        assert!(open_worker(&worker).is_ok());
        assert!(open_worker(Path::new("relative-worker")).is_err());

        std::fs::set_permissions(&worker, std::fs::Permissions::from_mode(0o755))
            .expect("worker mode must be set");
        assert!(open_worker(&worker).is_err());

        std::fs::set_permissions(&worker, std::fs::Permissions::from_mode(0o4555))
            .expect("worker mode must be set");
        assert!(open_worker(&worker).is_err());
    }

    struct TestPreviewTool {
        definition: ToolDefinition,
    }

    impl ContainedToolPort for TestPreviewTool {
        fn definition(&self) -> &ToolDefinition {
            &self.definition
        }
        fn approval_preview(
            &self,
            input: &agent_core::ToolInput,
        ) -> Result<ApprovalPreview, ContainmentPortError> {
            let input =
                parse_input(input.as_value()).map_err(|_| ContainmentPortError::PreviewRejected)?;
            ApprovalPreview::new(
                format!("workspace_write_file ({} bytes)", input.content.len()),
                input.relative_path,
            )
            .map_err(ContainmentPortError::from)
        }
        fn start_contained(
            &self,
            _call: ToolCall,
        ) -> Result<Box<dyn ContainedInvocation>, ContainmentPortError> {
            Err(ContainmentPortError::Unavailable)
        }
    }
}
