use std::{env, fmt, net::IpAddr, path::PathBuf, time::Duration};

use reqwest::{Client, StatusCode, header};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use url::{Host, Url};
use zeroize::Zeroize;

const MAX_RESPONSE_BYTES: usize = 64 * 1024;
const MAX_ITEMS: usize = 64;
const MAX_TEXT_BYTES: usize = 256;
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

fn bounded_text(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_TEXT_BYTES
        && value.chars().all(|character| !character.is_control())
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

    async fn get<T>(&self, path: &'static str) -> Result<T, LocalServiceError>
    where
        T: DeserializeOwned + BoundedResponse,
    {
        let mut request = match &self.transport {
            #[cfg(unix)]
            LocalServiceTransport::UnixSocket { .. } => {
                self.client.get(format!("http://localhost/{path}"))
            }
            LocalServiceTransport::Loopback {
                base_url,
                bearer,
                origin,
            } => {
                let url = base_url
                    .join(path)
                    .map_err(|_| LocalServiceError::InvalidConfiguration)?;
                self.client
                    .get(url)
                    .bearer_auth(bearer.expose())
                    .header(header::ORIGIN, origin.clone())
            }
        };
        request = request.header(header::ACCEPT, "application/json");
        let mut response = request
            .send()
            .await
            .map_err(|_| LocalServiceError::Unavailable)?;
        if response.status() != StatusCode::OK {
            return Err(LocalServiceError::Unavailable);
        }
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
}

impl LocalServiceError {
    pub const fn code(self) -> &'static str {
        match self {
            Self::InvalidConfiguration => "invalid_service_configuration",
            Self::Unavailable => "service_unavailable",
            Self::ResponseTooLarge => "service_response_too_large",
            Self::InvalidResponse => "invalid_service_response",
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
}
