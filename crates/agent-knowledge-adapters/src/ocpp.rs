use agent_core::KnowledgeBackendId;
use agent_knowledge::{
    BackendEvidenceSet, Evidence, EvidenceId, EvidenceMetadata, EvidenceSource,
    KnowledgeBackendPort, KnowledgeError, KnowledgeFuture, KnowledgeRequest,
    MAX_MCP_RESPONSE_BYTES, NativeScore, NativeScoreSystem,
};
use reqwest::{Client, StatusCode, redirect::Policy};
use serde::Deserialize;
use url::Url;

const BACKEND: KnowledgeBackendId = KnowledgeBackendId::OcppRagKag;

#[derive(Clone, Debug)]
pub struct OcppApiConfig {
    search_endpoint: Url,
}

impl OcppApiConfig {
    pub fn new(base_url: Url) -> Result<Self, OcppApiConfigError> {
        if base_url.scheme() != "http"
            || base_url.cannot_be_a_base()
            || base_url.username() != ""
            || base_url.password().is_some()
            || base_url.query().is_some()
            || base_url.fragment().is_some()
        {
            return Err(OcppApiConfigError::InvalidBaseUrl);
        }
        let search_endpoint = base_url
            .join("search")
            .map_err(|_| OcppApiConfigError::InvalidBaseUrl)?;
        Ok(Self { search_endpoint })
    }
}

pub struct OcppKnowledgeAdapter {
    client: Client,
    config: OcppApiConfig,
}

impl OcppKnowledgeAdapter {
    pub fn new(config: OcppApiConfig) -> Result<Self, OcppApiConfigError> {
        let client = Client::builder()
            .redirect(Policy::none())
            .build()
            .map_err(|_| OcppApiConfigError::ClientBuild)?;
        Ok(Self { client, config })
    }

    async fn retrieve(
        &self,
        request: KnowledgeRequest,
    ) -> Result<BackendEvidenceSet, KnowledgeError> {
        if request.snapshot_for(BACKEND).is_some() {
            return Err(KnowledgeError::SnapshotMismatch);
        }
        let mut endpoint = self.config.search_endpoint.clone();
        endpoint
            .query_pairs_mut()
            .append_pair("q", request.query().as_str())
            .append_pair("top_k", &request.limits().max_results().to_string())
            .append_pair("max_chars", &request.limits().max_item_bytes().to_string())
            .append_pair("include_content", "true")
            .append_pair("include_query", "false");
        let response = self
            .client
            .get(endpoint)
            .send()
            .await
            .map_err(|_| KnowledgeError::Unavailable(BACKEND))?;
        if response.status() != StatusCode::OK {
            return Err(if response.status().is_client_error() {
                KnowledgeError::Rejected(BACKEND)
            } else {
                KnowledgeError::Unavailable(BACKEND)
            });
        }
        if response
            .content_length()
            .is_some_and(|length| length > MAX_MCP_RESPONSE_BYTES as u64)
        {
            return Err(KnowledgeError::MalformedResponse);
        }
        let bytes = bounded_body(response).await?;
        let payload: SearchResponse =
            serde_json::from_slice(&bytes).map_err(|_| KnowledgeError::MalformedResponse)?;
        normalize(payload, request.limits().max_item_bytes())
    }
}

impl KnowledgeBackendPort for OcppKnowledgeAdapter {
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

async fn bounded_body(mut response: reqwest::Response) -> Result<Vec<u8>, KnowledgeError> {
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| KnowledgeError::Unavailable(BACKEND))?
    {
        if body.len().saturating_add(chunk.len()) > MAX_MCP_RESPONSE_BYTES {
            return Err(KnowledgeError::MalformedResponse);
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

fn normalize(
    payload: SearchResponse,
    max_item_bytes: usize,
) -> Result<BackendEvidenceSet, KnowledgeError> {
    if payload.results.len() > agent_knowledge::MAX_RESULTS || payload.correlation_id.is_empty() {
        return Err(KnowledgeError::MalformedResponse);
    }
    let mut truncated = false;
    let mut evidence = Vec::with_capacity(payload.results.len());
    for (index, item) in payload.results.into_iter().enumerate() {
        let content = item.content.ok_or(KnowledgeError::MalformedResponse)?;
        let (content, was_truncated) = truncate_utf8(content, max_item_bytes);
        truncated |= was_truncated;
        let reference_id = format!("{}:{}", item.document_id, item.chunk_id);
        let metadata = EvidenceMetadata::Ocpp {
            strategy: item.strategy,
            section_title: item.section_title,
            page_start: item.page_start,
            page_end: item.page_end,
            evidence_layer: item.evidence_layer,
            source_type: item.source_type,
        };
        let normalized = Evidence::new(
            EvidenceId::new(format!("ocpp:{}", item.chunk_id))
                .map_err(|_| KnowledgeError::MalformedResponse)?,
            EvidenceSource::new(BACKEND, item.document_id.clone())
                .map_err(|_| KnowledgeError::MalformedResponse)?,
            content,
            index + 1,
            Some(
                NativeScore::new(item.score, NativeScoreSystem::OcppHybridRetrieval)
                    .map_err(|_| KnowledgeError::MalformedResponse)?,
            ),
            Some(item.document_id),
            Some(item.chunk_id),
            reference_id,
            None,
            item.content_hash,
            metadata,
        )
        .map_err(|_| KnowledgeError::MalformedResponse)?;
        evidence.push(normalized);
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
struct SearchResponse {
    correlation_id: String,
    results: Vec<ScoredChunk>,
}

#[derive(Deserialize)]
struct ScoredChunk {
    chunk_id: String,
    document_id: String,
    content: Option<String>,
    score: f64,
    strategy: String,
    section_title: Option<String>,
    page_start: Option<u32>,
    page_end: Option<u32>,
    evidence_layer: Option<String>,
    source_type: Option<String>,
    content_hash: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OcppApiConfigError {
    InvalidBaseUrl,
    ClientBuild,
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };

    const FIXTURE: &str = r#"{
      "correlation_id":"c-1","query":null,"query_ref":{"sha256":"redacted"},
      "results":[{"chunk_id":"11111111-1111-1111-1111-111111111111",
        "document_id":"22222222-2222-2222-2222-222222222222",
        "content":"OCPP evidence","score":0.75,"strategy":"hybrid",
        "section_title":"ChargingProfile","page_start":10,"page_end":11,
        "evidence_layer":"spec","source_type":"spec_pdf","source_path":"/private/spec.pdf",
        "content_hash":"sha256:abc","semantic_links":[]}],
      "strategy_breakdown":{"hybrid":1},"latency_ms":7}"#;

    #[test]
    fn exact_search_fixture_normalizes_without_exposing_source_path() {
        let payload: SearchResponse = serde_json::from_str(FIXTURE).expect("fixture");
        let set = normalize(payload, 4096).expect("normalize");
        assert_eq!(set.backend(), BACKEND);
        assert_eq!(set.evidence().len(), 1);
        let item = &set.evidence()[0];
        assert_eq!(item.content(), "OCPP evidence");
        assert_eq!(
            item.native_score().expect("score").system(),
            NativeScoreSystem::OcppHybridRetrieval
        );
        assert!(!format!("{item:?}").contains("/private/spec.pdf"));
    }

    #[test]
    fn malformed_fixture_is_rejected() {
        let payload: SearchResponse = serde_json::from_str(
            r#"{"correlation_id":"c","results":[{"chunk_id":"c","document_id":"d","content":null,"score":1.0,"strategy":"hybrid"}]}"#,
        )
        .expect("fixture shape");
        assert_eq!(
            normalize(payload, 4096),
            Err(KnowledgeError::MalformedResponse)
        );
    }

    #[tokio::test]
    async fn unsupported_snapshot_fails_before_network_access() {
        let adapter = OcppKnowledgeAdapter::new(
            OcppApiConfig::new(Url::parse("http://127.0.0.1:9/").expect("url")).expect("config"),
        )
        .expect("adapter");
        let request = KnowledgeRequest::new(
            agent_knowledge::KnowledgeQuery::new("query").expect("query"),
            agent_knowledge::KnowledgeRoute::single(BACKEND),
            agent_knowledge::RetrievalLimits::default(),
        )
        .with_snapshot_requirements(vec![
            agent_knowledge::SnapshotRequirement::new(BACKEND, "snapshot-1").expect("snapshot"),
        ])
        .expect("request");
        assert_eq!(
            adapter.retrieve(request).await,
            Err(KnowledgeError::SnapshotMismatch)
        );
    }

    #[tokio::test]
    async fn adapter_uses_only_the_fixed_search_route_and_normalizes_response() {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("address");
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.expect("accept");
            let mut request = vec![0; 4096];
            let read = stream.read(&mut request).await.expect("read");
            let request = String::from_utf8(request[..read].to_vec()).expect("HTTP request");
            assert!(request.starts_with("GET /search?"));
            assert!(request.contains("top_k=8"));
            assert!(request.contains("include_query=false"));
            assert!(!request.to_ascii_lowercase().contains("authorization:"));
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                FIXTURE.len(),
                FIXTURE
            );
            stream.write_all(response.as_bytes()).await.expect("write");
        });
        let adapter = OcppKnowledgeAdapter::new(
            OcppApiConfig::new(Url::parse(&format!("http://{address}/")).expect("url"))
                .expect("config"),
        )
        .expect("adapter");
        let request = KnowledgeRequest::new(
            agent_knowledge::KnowledgeQuery::new("charging profile").expect("query"),
            agent_knowledge::KnowledgeRoute::single(BACKEND),
            agent_knowledge::RetrievalLimits::default(),
        );

        let set = adapter.retrieve(request).await.expect("retrieve");
        assert_eq!(set.evidence().len(), 1);
        server.await.expect("server");
    }
}
