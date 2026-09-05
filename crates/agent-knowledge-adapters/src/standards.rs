use std::{ffi::OsString, fmt, path::PathBuf, process::Stdio};

use agent_core::KnowledgeBackendId;
use agent_knowledge::{
    BackendEvidenceSet, Evidence, EvidenceId, EvidenceMetadata, EvidenceSource,
    KnowledgeBackendPort, KnowledgeError, KnowledgeFuture, KnowledgeRequest,
    MAX_MCP_RESPONSE_BYTES, NativeScore, NativeScoreSystem,
};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, ChildStdout, Command},
};

const BACKEND: KnowledgeBackendId = KnowledgeBackendId::StandardsMcp;
const TOOL_NAME: &str = "search_standards_kag";
const MCP_PROTOCOL_VERSION: &str = "2025-06-18";
const MAX_PROTOCOL_MESSAGES: usize = 32;

#[derive(Clone)]
pub struct StandardsMcpConfig {
    executable: PathBuf,
    arguments: Vec<OsString>,
    environment: Vec<(OsString, OsString)>,
}

impl fmt::Debug for StandardsMcpConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let environment_keys: Vec<_> = self
            .environment
            .iter()
            .map(|(key, _)| key.to_string_lossy())
            .collect();
        formatter
            .debug_struct("StandardsMcpConfig")
            .field("executable", &self.executable)
            .field("argument_count", &self.arguments.len())
            .field("environment_keys", &environment_keys)
            .field("environment_values", &"[REDACTED]")
            .finish()
    }
}

impl StandardsMcpConfig {
    pub fn new(
        executable: PathBuf,
        arguments: Vec<OsString>,
        environment: Vec<(OsString, OsString)>,
    ) -> Result<Self, StandardsMcpConfigError> {
        if !executable.is_absolute()
            || arguments.len() > 16
            || environment.len() > 16
            || arguments
                .iter()
                .any(|value| value.to_string_lossy().len() > 4096)
            || environment.iter().any(|(key, value)| {
                key.to_string_lossy().len() > 4096 || value.to_string_lossy().len() > 4096
            })
        {
            return Err(StandardsMcpConfigError::InvalidProcessConfig);
        }
        Ok(Self {
            executable,
            arguments,
            environment,
        })
    }
}

pub struct StandardsMcpKnowledgeAdapter {
    config: StandardsMcpConfig,
}

impl StandardsMcpKnowledgeAdapter {
    #[must_use]
    pub const fn new(config: StandardsMcpConfig) -> Self {
        Self { config }
    }

    async fn retrieve(
        &self,
        request: KnowledgeRequest,
    ) -> Result<BackendEvidenceSet, KnowledgeError> {
        if request.snapshot_for(BACKEND).is_some() {
            return Err(KnowledgeError::SnapshotMismatch);
        }
        let mut session = McpSession::spawn(&self.config).await?;
        let result = async {
            session.initialize().await?;
            let result = session
                .call_search(request.query().as_str(), request.limits().max_results())
                .await?;
            normalize(result, request.limits().max_item_bytes())
        }
        .await;
        session.kill_and_reap().await;
        result
    }
}

impl KnowledgeBackendPort for StandardsMcpKnowledgeAdapter {
    fn backend(&self) -> KnowledgeBackendId {
        BACKEND
    }

    fn retrieve_backend<'a>(
        &'a self,
        request: KnowledgeRequest,
    ) -> KnowledgeFuture<'a, Result<BackendEvidenceSet, KnowledgeError>> {
        Box::pin(self.retrieve(request))
    }
}

struct McpSession {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    bytes_read: usize,
}

impl McpSession {
    async fn spawn(config: &StandardsMcpConfig) -> Result<Self, KnowledgeError> {
        let mut command = Command::new(&config.executable);
        command
            .args(&config.arguments)
            .env_clear()
            .envs(config.environment.iter().cloned())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        let mut child = command
            .spawn()
            .map_err(|_| KnowledgeError::Unavailable(BACKEND))?;
        let stdin = child
            .stdin
            .take()
            .ok_or(KnowledgeError::Unavailable(BACKEND))?;
        let stdout = child
            .stdout
            .take()
            .ok_or(KnowledgeError::Unavailable(BACKEND))?;
        Ok(Self {
            child,
            stdin,
            stdout: BufReader::new(stdout),
            bytes_read: 0,
        })
    }

    async fn initialize(&mut self) -> Result<(), KnowledgeError> {
        self.send(&json!({
            "jsonrpc":"2.0", "id":1, "method":"initialize",
            "params": {
                "protocolVersion": MCP_PROTOCOL_VERSION,
                "capabilities": {},
                "clientInfo": {"name":"enterprise-local-agent", "version":"0.1.0"}
            }
        }))
        .await?;
        let response = self.response(1).await?;
        let version = response
            .get("result")
            .and_then(|result| result.get("protocolVersion"))
            .and_then(Value::as_str)
            .ok_or(KnowledgeError::MalformedResponse)?;
        if version != MCP_PROTOCOL_VERSION {
            return Err(KnowledgeError::MalformedResponse);
        }
        self.send(&json!({
            "jsonrpc":"2.0", "method":"notifications/initialized", "params":{}
        }))
        .await
    }

    async fn call_search(
        &mut self,
        query: &str,
        limit: usize,
    ) -> Result<StandardsResponse, KnowledgeError> {
        self.send(&search_call(query, limit)).await?;
        let response = self.response(2).await?;
        let result = response
            .get("result")
            .ok_or(KnowledgeError::MalformedResponse)?;
        if result.get("isError").and_then(Value::as_bool) == Some(true) {
            return Err(KnowledgeError::Failed(BACKEND));
        }
        let structured = result
            .get("structuredContent")
            .ok_or(KnowledgeError::MalformedResponse)?;
        serde_json::from_value(structured.clone()).map_err(|_| KnowledgeError::MalformedResponse)
    }

    async fn send(&mut self, value: &Value) -> Result<(), KnowledgeError> {
        let mut bytes = serde_json::to_vec(value).map_err(|_| KnowledgeError::Failed(BACKEND))?;
        bytes.push(b'\n');
        self.stdin
            .write_all(&bytes)
            .await
            .map_err(|_| KnowledgeError::Unavailable(BACKEND))?;
        self.stdin
            .flush()
            .await
            .map_err(|_| KnowledgeError::Unavailable(BACKEND))
    }

    async fn response(&mut self, expected_id: u64) -> Result<Value, KnowledgeError> {
        for _ in 0..MAX_PROTOCOL_MESSAGES {
            let line = self.read_bounded_line().await?;
            let value: Value =
                serde_json::from_slice(&line).map_err(|_| KnowledgeError::MalformedResponse)?;
            if value.get("id").and_then(Value::as_u64) != Some(expected_id) {
                continue;
            }
            if value.get("error").is_some() {
                return Err(KnowledgeError::Failed(BACKEND));
            }
            return Ok(value);
        }
        Err(KnowledgeError::MalformedResponse)
    }

    async fn read_bounded_line(&mut self) -> Result<Vec<u8>, KnowledgeError> {
        let mut line = Vec::new();
        loop {
            let available = self
                .stdout
                .fill_buf()
                .await
                .map_err(|_| KnowledgeError::Unavailable(BACKEND))?;
            if available.is_empty() {
                return Err(KnowledgeError::Unavailable(BACKEND));
            }
            let consumed = available
                .iter()
                .position(|byte| *byte == b'\n')
                .map_or(available.len(), |index| index + 1);
            if self.bytes_read.saturating_add(consumed) > MAX_MCP_RESPONSE_BYTES {
                return Err(KnowledgeError::MalformedResponse);
            }
            line.extend_from_slice(&available[..consumed]);
            self.stdout.consume(consumed);
            self.bytes_read += consumed;
            if line.last() == Some(&b'\n') {
                line.pop();
                return Ok(line);
            }
        }
    }

    async fn kill_and_reap(&mut self) {
        let _ = self.child.kill().await;
        let _ = self.child.wait().await;
    }
}

fn search_call(query: &str, limit: usize) -> Value {
    json!({
        "jsonrpc":"2.0", "id":2, "method":"tools/call",
        "params": {
            "name": TOOL_NAME,
            "arguments": {
                "query": query,
                "limit": limit,
                "include_preview": true,
                "provider": "local",
                "model": "BAAI/bge-m3",
                "pool_limit": 20,
                "graph_weight": 0.001,
                "review_status": null
            }
        }
    })
}

fn normalize(
    payload: StandardsResponse,
    max_item_bytes: usize,
) -> Result<BackendEvidenceSet, KnowledgeError> {
    if payload.retrieval_mode != "hybrid_rrf_light_kag"
        || payload.results.len() > agent_knowledge::MAX_RESULTS
    {
        return Err(KnowledgeError::MalformedResponse);
    }
    let mut evidence = Vec::with_capacity(payload.results.len());
    let mut truncated = false;
    for (index, item) in payload.results.into_iter().enumerate() {
        let content = item
            .content_preview
            .ok_or(KnowledgeError::MalformedResponse)?;
        let (content, was_truncated) = truncate_utf8(content, max_item_bytes);
        truncated |= was_truncated;
        let page_range = match (item.page_range.start, item.page_range.end) {
            (None, None) => None,
            (start, end) => Some(format!(
                "{}-{}",
                start.map_or_else(String::new, |value| value.to_string()),
                end.map_or_else(String::new, |value| value.to_string())
            )),
        };
        let reference_id = format!("{}:{}", item.source_id, item.chunk_id);
        evidence.push(
            Evidence::new(
                EvidenceId::new(format!("standards:{}", item.chunk_id))
                    .map_err(|_| KnowledgeError::MalformedResponse)?,
                EvidenceSource::new(BACKEND, item.source_id.clone())
                    .map_err(|_| KnowledgeError::MalformedResponse)?,
                content,
                index + 1,
                Some(
                    NativeScore::new(item.combined_score, NativeScoreSystem::StandardsKagCombined)
                        .map_err(|_| KnowledgeError::MalformedResponse)?,
                ),
                Some(item.source_id),
                Some(item.chunk_id),
                reference_id,
                None,
                None,
                EvidenceMetadata::Standards {
                    source_title: item.source_title,
                    section: item.section,
                    heading: item.heading,
                    page_range,
                    chunk_type: item.chunk_type,
                },
            )
            .map_err(|_| KnowledgeError::MalformedResponse)?,
        );
    }
    BackendEvidenceSet::new(BACKEND, evidence, None, truncated)
        .map_err(|_| KnowledgeError::MalformedResponse)
}

fn truncate_utf8(mut value: String, max_bytes: usize) -> (String, bool) {
    if value.len() <= max_bytes {
        return (value, false);
    }
    let mut boundary = max_bytes;
    while !value.is_char_boundary(boundary) {
        boundary -= 1;
    }
    value.truncate(boundary);
    (value, true)
}

#[derive(Deserialize)]
struct StandardsResponse {
    retrieval_mode: String,
    results: Vec<StandardsResult>,
}

#[derive(Deserialize)]
struct StandardsResult {
    chunk_id: String,
    source_id: String,
    source_title: String,
    section: Option<String>,
    heading: Option<String>,
    page_range: PageRange,
    chunk_type: String,
    combined_score: f64,
    content_preview: Option<String>,
}

#[derive(Deserialize)]
struct PageRange {
    start: Option<u32>,
    end: Option<u32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StandardsMcpConfigError {
    InvalidProcessConfig,
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_knowledge::{KnowledgeQuery, KnowledgeRoute, RetrievalLimits, SnapshotRequirement};

    const FIXTURE: &str = r#"{
      "query":"charging loop","source_id":null,"retrieval_mode":"hybrid_rrf_light_kag",
      "provider":"local","model":"BAAI/bge-m3","pool_limit":20,"graph_weight":0.001,
      "review_status":null,"results":[{
        "chunk_id":"chunk-1","source_id":"ISO15118-20","source_title":"ISO 15118-20",
        "section":"8.4","heading":"Charge loop","page_range":{"start":42,"end":43},
        "chunk_type":"text","combined_score":0.52,"hybrid_score":0.51,"graph_score":0.01,
        "keyword_rank":1,"vector_rank":2,"relationship_count":3,
        "reviewed_relationship_count":1,"related_entities":[],
        "content_preview":"Standards evidence"}] }"#;

    #[test]
    fn exact_kag_fixture_normalizes_with_provenance() {
        let payload: StandardsResponse = serde_json::from_str(FIXTURE).expect("fixture");
        let set = normalize(payload, 4096).expect("normalize");
        assert_eq!(set.evidence().len(), 1);
        let item = &set.evidence()[0];
        assert_eq!(item.source().source_id(), "ISO15118-20");
        assert_eq!(item.reference_id(), "ISO15118-20:chunk-1");
        assert_eq!(
            item.native_score().expect("score").system(),
            NativeScoreSystem::StandardsKagCombined
        );
    }

    #[test]
    fn malformed_fixture_is_rejected() {
        let payload: StandardsResponse =
            serde_json::from_str(r#"{"retrieval_mode":"unexpected","results":[]}"#)
                .expect("fixture");
        assert_eq!(
            normalize(payload, 4096),
            Err(KnowledgeError::MalformedResponse)
        );
    }

    #[tokio::test]
    async fn unsupported_snapshot_fails_before_process_start() {
        let config = StandardsMcpConfig::new(
            PathBuf::from("/definitely/not/a/standards-mcp-server"),
            Vec::new(),
            Vec::new(),
        )
        .expect("trusted process config");
        let adapter = StandardsMcpKnowledgeAdapter::new(config);
        let request = KnowledgeRequest::new(
            KnowledgeQuery::new("charging loop").expect("query"),
            KnowledgeRoute::single(BACKEND),
            RetrievalLimits::default(),
        )
        .with_snapshot_requirements(vec![
            SnapshotRequirement::new(BACKEND, "snapshot-1").expect("snapshot"),
        ])
        .expect("request");

        assert_eq!(
            adapter.retrieve_backend(request).await,
            Err(KnowledgeError::SnapshotMismatch)
        );
    }

    #[test]
    fn process_config_debug_redacts_environment_values() {
        let config = StandardsMcpConfig::new(
            PathBuf::from("/trusted/standards-mcp"),
            vec![OsString::from("serve")],
            vec![(
                OsString::from("DATABASE_URL"),
                OsString::from("secret-value"),
            )],
        )
        .expect("config");

        let debug = format!("{config:?}");
        assert!(debug.contains("DATABASE_URL"));
        assert!(!debug.contains("secret-value"));
    }

    #[test]
    fn mcp_request_exposes_only_the_allowlisted_kag_operation() {
        let request = search_call("charging loop", 8);
        assert_eq!(request["method"], "tools/call");
        assert_eq!(request["params"]["name"], TOOL_NAME);
        assert_eq!(request["params"]["arguments"]["limit"], 8);
        assert_eq!(request["params"]["arguments"]["provider"], "local");
        assert!(request["params"].get("endpoint").is_none());
        assert!(request["params"].get("command").is_none());
    }

    #[tokio::test]
    async fn oversized_line_is_rejected_before_json_decode() {
        use tokio::io::AsyncWriteExt;
        let (client, mut server) = tokio::io::duplex(MAX_MCP_RESPONSE_BYTES + 2);
        tokio::spawn(async move {
            let payload = vec![b'x'; MAX_MCP_RESPONSE_BYTES + 1];
            let _ = server.write_all(&payload).await;
        });
        let mut reader = BufReader::new(client);
        let mut total = 0usize;
        loop {
            let available = reader.fill_buf().await.expect("read");
            if available.is_empty() {
                break;
            }
            total += available.len();
            let consumed = available.len();
            reader.consume(consumed);
            if total > MAX_MCP_RESPONSE_BYTES {
                break;
            }
        }
        assert!(total > MAX_MCP_RESPONSE_BYTES);
    }
}
