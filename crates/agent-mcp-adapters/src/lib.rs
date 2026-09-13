//! Governed stdio MCP executable-tool adapter.
//!
//! MCP is used only for discovery and transport. Runtime authority remains in
//! `agent-harness`; this crate can dispatch only configured read-only tools.

use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::OsString,
    fmt,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};

use agent_core::{
    CapabilityKind, ToolCall, ToolDefinition, ToolDomainFailure, ToolDomainFailureKind, ToolInput,
    ToolName, ToolOutput, ToolResult, ToolSchema,
};
use agent_harness::{
    ManagedToolInvocation, ManagedToolPort, PortFuture, ToolPortError, validate_tool_schema,
};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use thiserror::Error;
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, ChildStdout, Command},
    time::timeout,
};

pub const MCP_PROTOCOL_REVISION: &str = "2025-06-18";
pub const DEFAULT_MAX_MESSAGE_BYTES: usize = 256 * 1024;
pub const DEFAULT_MAX_RESULT_BYTES: usize = 64 * 1024;
pub const DEFAULT_MAX_PAGES: usize = 4;
pub const DEFAULT_MAX_TOOLS: usize = 64;
const MAX_CONFIG_ENTRIES: usize = 32;
const MAX_CONFIG_VALUE_BYTES: usize = 4096;
const MAX_PROTOCOL_MESSAGES: usize = 128;
const MAX_RESULT_PARTS: usize = 32;
const TERMINATION_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct McpServerId(String);

impl McpServerId {
    pub fn new(value: impl Into<String>) -> Result<Self, McpAdapterError> {
        let value = value.into();
        if value.is_empty()
            || value.len() > 128
            || value.trim() != value
            || value.chars().any(char::is_control)
        {
            return Err(McpAdapterError::InvalidConfiguration);
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct DefinitionFingerprint([u8; 32]);

impl DefinitionFingerprint {
    #[must_use]
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl fmt::Debug for DefinitionFingerprint {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("DefinitionFingerprint([REDACTED])")
    }
}

#[derive(Clone)]
pub struct StdioServerConfig {
    server_id: McpServerId,
    executable: PathBuf,
    arguments: Vec<OsString>,
    environment: Vec<(OsString, OsString)>,
    timeout: Duration,
    max_message_bytes: usize,
    max_result_bytes: usize,
    max_pages: usize,
    max_tools: usize,
}

impl fmt::Debug for StdioServerConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("StdioServerConfig")
            .field("server_id", &self.server_id)
            .field("executable", &self.executable)
            .field("argument_count", &self.arguments.len())
            .field(
                "environment_keys",
                &self
                    .environment
                    .iter()
                    .map(|(key, _)| key.to_string_lossy())
                    .collect::<Vec<_>>(),
            )
            .field("environment_values", &"[REDACTED]")
            .field("timeout", &self.timeout)
            .finish()
    }
}

impl StdioServerConfig {
    pub fn new(
        server_id: McpServerId,
        executable: PathBuf,
        arguments: Vec<OsString>,
        environment: Vec<(OsString, OsString)>,
        timeout: Duration,
    ) -> Result<Self, McpAdapterError> {
        validate_executable(&executable)?;
        if timeout.is_zero()
            || arguments.len() > MAX_CONFIG_ENTRIES
            || environment.len() > MAX_CONFIG_ENTRIES
            || arguments.iter().any(oversized_os_value)
            || environment
                .iter()
                .any(|(key, value)| oversized_os_value(key) || oversized_os_value(value))
        {
            return Err(McpAdapterError::InvalidConfiguration);
        }
        let mut keys = BTreeSet::new();
        if environment
            .iter()
            .any(|(key, _)| !keys.insert(key.clone()) || key.is_empty())
        {
            return Err(McpAdapterError::InvalidConfiguration);
        }
        Ok(Self {
            server_id,
            executable,
            arguments,
            environment,
            timeout,
            max_message_bytes: DEFAULT_MAX_MESSAGE_BYTES,
            max_result_bytes: DEFAULT_MAX_RESULT_BYTES,
            max_pages: DEFAULT_MAX_PAGES,
            max_tools: DEFAULT_MAX_TOOLS,
        })
    }

    #[must_use]
    pub fn with_limits(
        mut self,
        max_message_bytes: usize,
        max_result_bytes: usize,
        max_pages: usize,
        max_tools: usize,
    ) -> Self {
        self.max_message_bytes = max_message_bytes.clamp(1, DEFAULT_MAX_MESSAGE_BYTES);
        self.max_result_bytes = max_result_bytes.clamp(1, DEFAULT_MAX_RESULT_BYTES);
        self.max_pages = max_pages.clamp(1, DEFAULT_MAX_PAGES);
        self.max_tools = max_tools.clamp(1, DEFAULT_MAX_TOOLS);
        self
    }
}

#[derive(Clone, Debug)]
pub struct ToolMapping {
    remote_name: String,
    local_name: ToolName,
    trusted_description: String,
    capability: CapabilityKind,
    expected_fingerprint: DefinitionFingerprint,
}

impl ToolMapping {
    pub fn new(
        remote_name: impl Into<String>,
        local_name: ToolName,
        trusted_description: impl Into<String>,
        capability: CapabilityKind,
        expected_fingerprint: DefinitionFingerprint,
    ) -> Result<Self, McpAdapterError> {
        let remote_name = remote_name.into();
        let trusted_description = trusted_description.into();
        if remote_name.is_empty()
            || remote_name.len() > 255
            || remote_name.trim() != remote_name
            || remote_name.chars().any(char::is_control)
            || trusted_description.trim().is_empty()
            || trusted_description.len() > 1024
        {
            return Err(McpAdapterError::InvalidConfiguration);
        }
        Ok(Self {
            remote_name,
            local_name,
            trusted_description,
            capability,
            expected_fingerprint,
        })
    }
}

#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum McpAdapterError {
    #[error("MCP adapter configuration is invalid")]
    InvalidConfiguration,
    #[error("MCP server is unavailable")]
    Unavailable,
    #[error("MCP protocol negotiation failed")]
    ProtocolMismatch,
    #[error("MCP response was malformed or exceeded a bound")]
    MalformedResponse,
    #[error("MCP discovery exceeded a bound")]
    DiscoveryLimit,
    #[error("MCP discovery was ambiguous")]
    AmbiguousDiscovery,
    #[error("MCP tool is not supported by this runtime")]
    UnsupportedTool,
    #[error("MCP tool definition does not match trusted configuration")]
    DefinitionDrift,
    #[error("MCP operation timed out")]
    TimedOut,
}

#[derive(Clone)]
pub struct McpManagedTool {
    config: StdioServerConfig,
    mapping: ToolMapping,
    definition: ToolDefinition,
}

impl fmt::Debug for McpManagedTool {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("McpManagedTool")
            .field("config", &self.config)
            .field("local_name", self.definition.name())
            .field("capability", &self.definition.capability())
            .finish()
    }
}

/// Discovers exactly one allowlisted tool and verifies its configured snapshot.
pub async fn discover_tool(
    config: StdioServerConfig,
    mapping: ToolMapping,
) -> Result<McpManagedTool, McpAdapterError> {
    let mut session = Session::spawn(&config)?;
    let result = match timeout(config.timeout, async {
        session.initialize().await?;
        let remote = session.find_tool(&config, &mapping.remote_name).await?;
        let fingerprint = fingerprint(&remote)?;
        if fingerprint != mapping.expected_fingerprint {
            return Err(McpAdapterError::DefinitionDrift);
        }
        let schema =
            ToolSchema::new(remote.input_schema).map_err(|_| McpAdapterError::UnsupportedTool)?;
        validate_tool_schema(&schema).map_err(|_| McpAdapterError::UnsupportedTool)?;
        ToolDefinition::new(
            mapping.local_name.clone(),
            mapping.trusted_description.clone(),
            mapping.capability,
            schema,
        )
        .map_err(|_| McpAdapterError::InvalidConfiguration)
    })
    .await
    {
        Ok(result) => result,
        Err(_) => Err(McpAdapterError::TimedOut),
    };
    session.terminate_and_reap().await?;
    result.map(|definition| McpManagedTool {
        config,
        mapping,
        definition,
    })
}

impl ManagedToolPort for McpManagedTool {
    fn definition(&self) -> &ToolDefinition {
        &self.definition
    }

    fn start_managed(
        &self,
        call: ToolCall,
    ) -> Result<Box<dyn ManagedToolInvocation>, ToolPortError> {
        if self.definition.capability() != CapabilityKind::ReadOnly
            || call.name() != self.definition.name()
        {
            return Err(ToolPortError::AdapterFailure);
        }
        let session = Session::spawn(&self.config).map_err(|_| ToolPortError::Unavailable)?;
        Ok(Box::new(McpInvocation {
            session: Some(session),
            config: self.config.clone(),
            mapping: self.mapping.clone(),
            call: Some(call),
        }))
    }
}

struct McpInvocation {
    session: Option<Session>,
    config: StdioServerConfig,
    mapping: ToolMapping,
    call: Option<ToolCall>,
}

impl ManagedToolInvocation for McpInvocation {
    fn wait<'a>(&'a mut self) -> PortFuture<'a, Result<ToolResult, ToolPortError>> {
        Box::pin(async move {
            let call = self.call.take().ok_or(ToolPortError::AdapterFailure)?;
            let call_id = call.id();
            let input = call.input().clone();
            let result = {
                let session = self.session.as_mut().ok_or(ToolPortError::AdapterFailure)?;
                timeout(self.config.timeout, async {
                    session.initialize().await?;
                    let remote = session
                        .find_tool(&self.config, &self.mapping.remote_name)
                        .await?;
                    if fingerprint(&remote)? != self.mapping.expected_fingerprint {
                        return Err(McpAdapterError::DefinitionDrift);
                    }
                    session
                        .call_tool(
                            &self.mapping.remote_name,
                            &input,
                            self.config.max_result_bytes,
                        )
                        .await
                })
                .await
                .map_err(|_| McpAdapterError::TimedOut)
                .and_then(|result| result)
            };
            let cleanup = self.terminate_and_reap().await;
            cleanup?;
            map_result(call_id, result)
        })
    }

    fn terminate_and_reap<'a>(&'a mut self) -> PortFuture<'a, Result<(), ToolPortError>> {
        Box::pin(async move {
            if let Some(mut session) = self.session.take() {
                session
                    .terminate_and_reap()
                    .await
                    .map_err(|_| ToolPortError::AdapterFailure)?;
            }
            Ok(())
        })
    }
}

impl Drop for McpInvocation {
    fn drop(&mut self) {
        if let Some(session) = self.session.as_mut() {
            session.start_kill();
        }
    }
}

fn map_result(
    call_id: agent_core::ToolCallId,
    result: Result<RemoteCallResult, McpAdapterError>,
) -> Result<ToolResult, ToolPortError> {
    match result {
        Ok(RemoteCallResult::Success(output)) => Ok(ToolResult::Succeeded {
            call_id,
            output: ToolOutput::new(output),
        }),
        Ok(RemoteCallResult::DomainFailure) => Ok(ToolResult::DomainFailure {
            call_id,
            failure: ToolDomainFailure::new(ToolDomainFailureKind::Rejected),
        }),
        Err(McpAdapterError::Unavailable) => Err(ToolPortError::Unavailable),
        Err(_) => Err(ToolPortError::AdapterFailure),
    }
}

struct Session {
    child: Option<Child>,
    stdin: Option<ChildStdin>,
    stdout: Option<BufReader<ChildStdout>>,
    bytes_read: usize,
    max_message_bytes: usize,
    seen_response_ids: BTreeSet<u64>,
}

impl Session {
    fn spawn(config: &StdioServerConfig) -> Result<Self, McpAdapterError> {
        let mut command = Command::new(&config.executable);
        command
            .args(&config.arguments)
            .env_clear()
            .envs(config.environment.iter().cloned())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .process_group(0);
        let mut child = command.spawn().map_err(|_| McpAdapterError::Unavailable)?;
        let stdin = child.stdin.take().ok_or(McpAdapterError::Unavailable)?;
        let stdout = child.stdout.take().ok_or(McpAdapterError::Unavailable)?;
        Ok(Self {
            child: Some(child),
            stdin: Some(stdin),
            stdout: Some(BufReader::new(stdout)),
            bytes_read: 0,
            max_message_bytes: config.max_message_bytes,
            seen_response_ids: BTreeSet::new(),
        })
    }

    async fn initialize(&mut self) -> Result<(), McpAdapterError> {
        self.send(&json!({
            "jsonrpc":"2.0", "id":1, "method":"initialize",
            "params": {
                "protocolVersion": MCP_PROTOCOL_REVISION,
                "capabilities": {},
                "clientInfo": {"name":"enterprise-local-agent", "version":"0.1.0"}
            }
        }))
        .await?;
        let response = self.response(1).await?;
        let negotiated = response
            .get("result")
            .and_then(|result| result.get("protocolVersion"))
            .and_then(Value::as_str);
        if negotiated != Some(MCP_PROTOCOL_REVISION) {
            return Err(McpAdapterError::ProtocolMismatch);
        }
        self.send(&json!({
            "jsonrpc":"2.0", "method":"notifications/initialized", "params":{}
        }))
        .await
    }

    async fn find_tool(
        &mut self,
        config: &StdioServerConfig,
        wanted: &str,
    ) -> Result<RemoteTool, McpAdapterError> {
        let mut cursor: Option<String> = None;
        let mut cursors = BTreeSet::new();
        let mut names = BTreeSet::new();
        let mut found = None;
        let mut total = 0_usize;
        for page in 0..config.max_pages {
            let id = 2_u64 + u64::try_from(page).map_err(|_| McpAdapterError::DiscoveryLimit)?;
            let params = cursor
                .as_ref()
                .map_or_else(|| json!({}), |cursor| json!({"cursor":cursor}));
            self.send(&json!({"jsonrpc":"2.0", "id":id, "method":"tools/list", "params":params}))
                .await?;
            let response = self.response(id).await?;
            let result = response
                .get("result")
                .ok_or(McpAdapterError::MalformedResponse)?;
            let tools = result
                .get("tools")
                .and_then(Value::as_array)
                .ok_or(McpAdapterError::MalformedResponse)?;
            total = total.saturating_add(tools.len());
            if total > config.max_tools {
                return Err(McpAdapterError::DiscoveryLimit);
            }
            for value in tools {
                let tool = RemoteTool::parse(value)?;
                if !names.insert(tool.name.clone()) {
                    return Err(McpAdapterError::AmbiguousDiscovery);
                }
                if tool.name == wanted {
                    found = Some(tool);
                }
            }
            cursor = match result.get("nextCursor") {
                None | Some(Value::Null) => return found.ok_or(McpAdapterError::UnsupportedTool),
                Some(Value::String(value)) if !value.is_empty() => Some(value.clone()),
                _ => return Err(McpAdapterError::MalformedResponse),
            };
            if !cursors.insert(cursor.clone().ok_or(McpAdapterError::MalformedResponse)?) {
                return Err(McpAdapterError::AmbiguousDiscovery);
            }
        }
        Err(McpAdapterError::DiscoveryLimit)
    }

    async fn call_tool(
        &mut self,
        name: &str,
        input: &ToolInput,
        max_result_bytes: usize,
    ) -> Result<RemoteCallResult, McpAdapterError> {
        self.send(&json!({
            "jsonrpc":"2.0", "id":100, "method":"tools/call",
            "params":{"name":name, "arguments":input.as_value()}
        }))
        .await?;
        let response = self.response(100).await?;
        let result = response
            .get("result")
            .ok_or(McpAdapterError::MalformedResponse)?;
        if result
            .get("isError")
            .is_some_and(|value| !value.is_boolean())
        {
            return Err(McpAdapterError::MalformedResponse);
        }
        if result.get("isError").and_then(Value::as_bool) == Some(true) {
            return Ok(RemoteCallResult::DomainFailure);
        }
        normalize_result(result, max_result_bytes).map(RemoteCallResult::Success)
    }

    async fn send(&mut self, value: &Value) -> Result<(), McpAdapterError> {
        let mut bytes =
            serde_json::to_vec(value).map_err(|_| McpAdapterError::MalformedResponse)?;
        if bytes.len() >= self.max_message_bytes {
            return Err(McpAdapterError::MalformedResponse);
        }
        bytes.push(b'\n');
        let stdin = self.stdin.as_mut().ok_or(McpAdapterError::Unavailable)?;
        stdin
            .write_all(&bytes)
            .await
            .map_err(|_| McpAdapterError::Unavailable)?;
        stdin
            .flush()
            .await
            .map_err(|_| McpAdapterError::Unavailable)
    }

    async fn response(&mut self, expected_id: u64) -> Result<Value, McpAdapterError> {
        for _ in 0..MAX_PROTOCOL_MESSAGES {
            let line = self.read_line().await?;
            let value: Value =
                serde_json::from_slice(&line).map_err(|_| McpAdapterError::MalformedResponse)?;
            if value.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
                return Err(McpAdapterError::MalformedResponse);
            }
            let Some(id) = value.get("id").and_then(Value::as_u64) else {
                if value.get("method").and_then(Value::as_str).is_some() {
                    continue;
                }
                return Err(McpAdapterError::MalformedResponse);
            };
            if !self.seen_response_ids.insert(id) || id != expected_id {
                return Err(McpAdapterError::MalformedResponse);
            }
            if value.get("error").is_some() || value.get("result").is_none() {
                return Err(McpAdapterError::MalformedResponse);
            }
            return Ok(value);
        }
        Err(McpAdapterError::MalformedResponse)
    }

    async fn read_line(&mut self) -> Result<Vec<u8>, McpAdapterError> {
        let stdout = self.stdout.as_mut().ok_or(McpAdapterError::Unavailable)?;
        let mut line = Vec::new();
        loop {
            let available = stdout
                .fill_buf()
                .await
                .map_err(|_| McpAdapterError::Unavailable)?;
            if available.is_empty() {
                return Err(McpAdapterError::MalformedResponse);
            }
            let take = available
                .iter()
                .position(|byte| *byte == b'\n')
                .map_or(available.len(), |index| index + 1);
            if line.len().saturating_add(take) > self.max_message_bytes {
                return Err(McpAdapterError::MalformedResponse);
            }
            line.extend_from_slice(&available[..take]);
            stdout.consume(take);
            if line.ends_with(b"\n") {
                break;
            }
        }
        let read = line.len();
        self.bytes_read = self.bytes_read.saturating_add(read);
        if self.bytes_read > self.max_message_bytes.saturating_mul(MAX_PROTOCOL_MESSAGES) {
            return Err(McpAdapterError::MalformedResponse);
        }
        line.pop();
        std::str::from_utf8(&line).map_err(|_| McpAdapterError::MalformedResponse)?;
        Ok(line)
    }

    fn start_kill(&mut self) {
        if let Some(child) = self.child.as_mut() {
            signal_process_group(child);
            let _ = child.start_kill();
        }
    }

    async fn terminate_and_reap(&mut self) -> Result<(), McpAdapterError> {
        self.stdin.take();
        self.stdout.take();
        if let Some(mut child) = self.child.take() {
            signal_process_group(&mut child);
            let _ = child.start_kill();
            timeout(TERMINATION_TIMEOUT, child.wait())
                .await
                .map_err(|_| McpAdapterError::Unavailable)?
                .map_err(|_| McpAdapterError::Unavailable)?;
        }
        Ok(())
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.start_kill();
    }
}

fn signal_process_group(child: &mut Child) {
    let Some(pid) = child.id().and_then(|value| i32::try_from(value).ok()) else {
        return;
    };
    // SAFETY: `kill` is called with a negative, checked child PID to address
    // only the process group created for this invocation. No pointers cross FFI.
    unsafe {
        libc::kill(-pid, libc::SIGKILL);
    }
}

struct RemoteTool {
    name: String,
    input_schema: Value,
    definition: Value,
}

impl RemoteTool {
    fn parse(value: &Value) -> Result<Self, McpAdapterError> {
        let object = value
            .as_object()
            .ok_or(McpAdapterError::MalformedResponse)?;
        let name = object
            .get("name")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty() && value.len() <= 255)
            .ok_or(McpAdapterError::MalformedResponse)?
            .to_owned();
        let input_schema = object
            .get("inputSchema")
            .filter(|value| value.is_object())
            .ok_or(McpAdapterError::MalformedResponse)?
            .clone();
        if object.contains_key("outputSchema") {
            return Err(McpAdapterError::UnsupportedTool);
        }
        Ok(Self {
            name,
            input_schema,
            definition: value.clone(),
        })
    }
}

fn fingerprint(tool: &RemoteTool) -> Result<DefinitionFingerprint, McpAdapterError> {
    let canonical = canonicalize(&tool.definition);
    let bytes = serde_json::to_vec(&canonical).map_err(|_| McpAdapterError::MalformedResponse)?;
    Ok(DefinitionFingerprint(Sha256::digest(bytes).into()))
}

/// Computes the snapshot fingerprint for one raw `tools/list` tool object.
pub fn definition_fingerprint(
    tool_definition: &Value,
) -> Result<DefinitionFingerprint, McpAdapterError> {
    fingerprint(&RemoteTool::parse(tool_definition)?)
}

fn canonicalize(value: &Value) -> Value {
    match value {
        Value::Object(object) => Value::Object(
            object
                .iter()
                .map(|(key, value)| (key.clone(), canonicalize(value)))
                .collect::<BTreeMap<_, _>>()
                .into_iter()
                .collect::<Map<_, _>>(),
        ),
        Value::Array(values) => Value::Array(values.iter().map(canonicalize).collect()),
        _ => value.clone(),
    }
}

enum RemoteCallResult {
    Success(Value),
    DomainFailure,
}

fn normalize_result(result: &Value, max_bytes: usize) -> Result<Value, McpAdapterError> {
    let normalized = if let Some(structured) = result.get("structuredContent") {
        if !structured.is_object() && !structured.is_array() {
            return Err(McpAdapterError::MalformedResponse);
        }
        structured.clone()
    } else {
        let content = result
            .get("content")
            .and_then(Value::as_array)
            .ok_or(McpAdapterError::MalformedResponse)?;
        if content.len() > MAX_RESULT_PARTS {
            return Err(McpAdapterError::MalformedResponse);
        }
        let mut texts = Vec::with_capacity(content.len());
        for part in content {
            let object = part.as_object().ok_or(McpAdapterError::MalformedResponse)?;
            if object.get("type").and_then(Value::as_str) != Some("text") {
                return Err(McpAdapterError::UnsupportedTool);
            }
            texts.push(
                object
                    .get("text")
                    .and_then(Value::as_str)
                    .ok_or(McpAdapterError::MalformedResponse)?,
            );
        }
        json!({"text":texts})
    };
    let size = serde_json::to_vec(&normalized)
        .map_err(|_| McpAdapterError::MalformedResponse)?
        .len();
    if size > max_bytes {
        return Err(McpAdapterError::MalformedResponse);
    }
    Ok(normalized)
}

fn validate_executable(path: &Path) -> Result<(), McpAdapterError> {
    if !path.is_absolute() {
        return Err(McpAdapterError::InvalidConfiguration);
    }
    let metadata = path
        .symlink_metadata()
        .map_err(|_| McpAdapterError::InvalidConfiguration)?;
    let mode = metadata.permissions().mode();
    if !metadata.file_type().is_file()
        || mode & 0o111 == 0
        || mode & 0o022 != 0
        || mode & 0o6000 != 0
    {
        return Err(McpAdapterError::InvalidConfiguration);
    }
    Ok(())
}

fn oversized_os_value(value: &OsString) -> bool {
    value.len() > MAX_CONFIG_VALUE_BYTES || value.is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_fingerprint_is_object_order_independent() {
        let first = RemoteTool::parse(&json!({
            "name":"lookup", "description":"remote", "inputSchema":{"type":"object"}
        }))
        .expect("fixture is valid");
        let second = RemoteTool::parse(&json!({
            "inputSchema":{"type":"object"}, "description":"remote", "name":"lookup"
        }))
        .expect("fixture is valid");
        assert_eq!(fingerprint(&first), fingerprint(&second));
    }

    #[test]
    fn rich_results_are_rejected() {
        assert_eq!(
            normalize_result(
                &json!({"content":[{"type":"image","data":"payload"}]}),
                1024
            ),
            Err(McpAdapterError::UnsupportedTool)
        );
    }
}
