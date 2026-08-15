use std::{fmt, sync::Arc};

use agent_harness::ModelPort;
use rig_core::{client::CompletionClient, providers::openai::CompletionsClient};
use thiserror::Error;
use url::{Host, Url};

use crate::RigModelAdapter;

const MAX_PROVIDER_LABEL_LEN: usize = 64;

/// Bearer credential used to construct an OpenAI-compatible Rig client.
///
/// The secret is intentionally not serializable and has no `Display` or public
/// raw-value accessor.
///
/// ```compile_fail
/// use agent_provider_rig::BearerCredential;
///
/// let credential = BearerCredential::new("secret").unwrap();
/// let _ = rig_core::serde_json::to_string(&credential);
/// ```
pub struct BearerCredential(String);

impl BearerCredential {
    pub fn new(value: impl Into<String>) -> Result<Self, BearerCredentialError> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(BearerCredentialError::Empty);
        }

        Ok(Self(value))
    }

    fn into_inner(self) -> String {
        self.0
    }
}

impl fmt::Debug for BearerCredential {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("BearerCredential(<redacted>)")
    }
}

#[derive(Clone, Copy, Debug, Error, PartialEq, Eq)]
pub enum BearerCredentialError {
    #[error("bearer credential must not be empty")]
    Empty,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProviderLabel(String);

impl ProviderLabel {
    pub fn new(value: impl Into<String>) -> Result<Self, ProviderLabelError> {
        let value = value.into();
        if value.is_empty()
            || value.len() > MAX_PROVIDER_LABEL_LEN
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
        {
            return Err(ProviderLabelError::Invalid);
        }

        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Copy, Debug, Error, PartialEq, Eq)]
pub enum ProviderLabelError {
    #[error("provider label is invalid")]
    Invalid,
}

pub struct OpenAiCompatibleConfig {
    base_url: Url,
    model_identifier: String,
    credential: BearerCredential,
    provider_label: Option<ProviderLabel>,
}

impl OpenAiCompatibleConfig {
    pub fn new(
        base_url: impl AsRef<str>,
        model_identifier: impl Into<String>,
        credential: BearerCredential,
    ) -> Result<Self, OpenAiCompatibleConfigError> {
        let mut base_url = Url::parse(base_url.as_ref())
            .map_err(|_| OpenAiCompatibleConfigError::InvalidBaseUrl)?;
        validate_endpoint(&base_url)?;
        normalize_optional_trailing_slash(&mut base_url);

        let model_identifier = model_identifier.into();
        if model_identifier.trim().is_empty() || model_identifier.chars().any(char::is_control) {
            return Err(OpenAiCompatibleConfigError::InvalidModelIdentifier);
        }

        Ok(Self {
            base_url,
            model_identifier,
            credential,
            provider_label: None,
        })
    }

    #[must_use]
    pub fn with_provider_label(mut self, provider_label: ProviderLabel) -> Self {
        self.provider_label = Some(provider_label);
        self
    }

    #[must_use]
    pub fn provider_label(&self) -> Option<&ProviderLabel> {
        self.provider_label.as_ref()
    }
}

#[derive(Clone, Copy, Debug, Error, PartialEq, Eq)]
pub enum OpenAiCompatibleConfigError {
    #[error("OpenAI-compatible base URL is invalid")]
    InvalidBaseUrl,
    #[error("OpenAI-compatible base URL scheme must be HTTP or HTTPS")]
    UnsupportedScheme,
    #[error("OpenAI-compatible base URL must contain a host")]
    MissingHost,
    #[error("OpenAI-compatible base URL must not contain credentials")]
    UrlCredentialsNotAllowed,
    #[error("OpenAI-compatible base URL must not contain a query")]
    QueryNotAllowed,
    #[error("OpenAI-compatible base URL must not contain a fragment")]
    FragmentNotAllowed,
    #[error("OpenAI-compatible base URL must name an API root, not chat completions")]
    CompletionEndpointNotAllowed,
    #[error("unencrypted HTTP is allowed only for loopback endpoints")]
    InsecureRemoteHttp,
    #[error("OpenAI-compatible model identifier is invalid")]
    InvalidModelIdentifier,
}

#[derive(Clone, Copy, Debug, Error, PartialEq, Eq)]
pub enum OpenAiCompatibleBuildError {
    #[error("OpenAI-compatible model client construction failed")]
    ClientConstructionFailed,
}

pub fn build_openai_compatible_model_port(
    config: OpenAiCompatibleConfig,
) -> Result<Arc<dyn ModelPort>, OpenAiCompatibleBuildError> {
    let OpenAiCompatibleConfig {
        base_url,
        model_identifier,
        credential,
        provider_label: _,
    } = config;

    let client = CompletionsClient::builder()
        .api_key(credential.into_inner())
        .base_url(base_url.as_str())
        .build()
        .map_err(|_| OpenAiCompatibleBuildError::ClientConstructionFailed)?;
    let model = client.completion_model(model_identifier);

    Ok(Arc::new(RigModelAdapter::new(model)))
}

fn validate_endpoint(base_url: &Url) -> Result<(), OpenAiCompatibleConfigError> {
    if base_url.host().is_none() {
        return Err(OpenAiCompatibleConfigError::MissingHost);
    }
    if !matches!(base_url.scheme(), "http" | "https") {
        return Err(OpenAiCompatibleConfigError::UnsupportedScheme);
    }
    if !base_url.username().is_empty() || base_url.password().is_some() {
        return Err(OpenAiCompatibleConfigError::UrlCredentialsNotAllowed);
    }
    if base_url.query().is_some() {
        return Err(OpenAiCompatibleConfigError::QueryNotAllowed);
    }
    if base_url.fragment().is_some() {
        return Err(OpenAiCompatibleConfigError::FragmentNotAllowed);
    }

    let path_without_optional_slash = base_url.path().strip_suffix('/').unwrap_or(base_url.path());
    if path_without_optional_slash.ends_with("/chat/completions") {
        return Err(OpenAiCompatibleConfigError::CompletionEndpointNotAllowed);
    }

    if base_url.scheme() == "http" && !is_loopback(base_url) {
        return Err(OpenAiCompatibleConfigError::InsecureRemoteHttp);
    }

    Ok(())
}

fn is_loopback(url: &Url) -> bool {
    match url.host() {
        Some(Host::Domain(domain)) => domain.eq_ignore_ascii_case("localhost"),
        Some(Host::Ipv4(address)) => address.is_loopback(),
        Some(Host::Ipv6(address)) => address.is_loopback(),
        None => false,
    }
}

fn normalize_optional_trailing_slash(base_url: &mut Url) {
    let path = base_url.path();
    if path != "/" && path.ends_with('/') {
        let normalized = path[..path.len() - 1].to_owned();
        base_url.set_path(&normalized);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn credential() -> BearerCredential {
        BearerCredential::new("test-secret").expect("test credential must be valid")
    }

    #[test]
    fn credential_rejects_empty_values_and_redacts_debug() {
        assert_eq!(
            BearerCredential::new("   ").err(),
            Some(BearerCredentialError::Empty)
        );

        let secret = "credential-debug-sentinel";
        let credential = BearerCredential::new(secret).expect("credential must be valid");
        let rendered = format!("{credential:?}");

        assert_eq!(rendered, "BearerCredential(<redacted>)");
        assert!(!rendered.contains(secret));
    }

    #[test]
    fn provider_label_is_bounded_and_safe() {
        assert_eq!(
            ProviderLabel::new("local-qwen")
                .expect("label must be valid")
                .as_str(),
            "local-qwen"
        );
        for invalid in ["", "contains space", "contains\nnewline"] {
            assert_eq!(
                ProviderLabel::new(invalid).err(),
                Some(ProviderLabelError::Invalid)
            );
        }
        assert_eq!(
            ProviderLabel::new("a".repeat(MAX_PROVIDER_LABEL_LEN + 1)).err(),
            Some(ProviderLabelError::Invalid)
        );
    }

    #[test]
    fn endpoint_validation_accepts_only_loopback_http() {
        for endpoint in [
            "http://localhost:8000/v1",
            "http://LOCALHOST:8000/v1/",
            "http://127.0.0.1:8000/v1",
            "http://[::1]:8000/v1",
            "https://gateway.internal.example/v1",
        ] {
            OpenAiCompatibleConfig::new(endpoint, "opaque-model", credential())
                .expect("endpoint must be accepted");
        }

        for endpoint in [
            "http://0.0.0.0:8000/v1",
            "http://10.0.0.4:8000/v1",
            "http://192.168.1.4:8000/v1",
            "http://gateway.internal.example/v1",
        ] {
            assert_eq!(
                OpenAiCompatibleConfig::new(endpoint, "opaque-model", credential()).err(),
                Some(OpenAiCompatibleConfigError::InsecureRemoteHttp)
            );
        }
    }

    #[test]
    fn endpoint_validation_rejects_unsafe_or_non_root_urls() {
        let cases = [
            ("not a URL", OpenAiCompatibleConfigError::InvalidBaseUrl),
            (
                "ftp://localhost/v1",
                OpenAiCompatibleConfigError::UnsupportedScheme,
            ),
            ("file:///v1", OpenAiCompatibleConfigError::MissingHost),
            (
                "http://user:password@localhost/v1",
                OpenAiCompatibleConfigError::UrlCredentialsNotAllowed,
            ),
            (
                "http://localhost/v1?token=secret",
                OpenAiCompatibleConfigError::QueryNotAllowed,
            ),
            (
                "http://localhost/v1#fragment",
                OpenAiCompatibleConfigError::FragmentNotAllowed,
            ),
            (
                "http://localhost/v1/chat/completions",
                OpenAiCompatibleConfigError::CompletionEndpointNotAllowed,
            ),
            (
                "http://localhost/v1/chat/completions/",
                OpenAiCompatibleConfigError::CompletionEndpointNotAllowed,
            ),
        ];

        for (endpoint, expected) in cases {
            assert_eq!(
                OpenAiCompatibleConfig::new(endpoint, "opaque-model", credential()).err(),
                Some(expected)
            );
        }
    }

    #[test]
    fn model_identifier_is_preserved_and_validated() {
        let identifier = "code_chat@latest?context_size > 100000";
        let config =
            OpenAiCompatibleConfig::new("http://localhost:8000/v1", identifier, credential())
                .expect("model identifier must be valid");

        assert_eq!(config.model_identifier, identifier);
        for invalid in ["", "   ", "model\nidentifier"] {
            assert_eq!(
                OpenAiCompatibleConfig::new("http://localhost:8000/v1", invalid, credential(),)
                    .err(),
                Some(OpenAiCompatibleConfigError::InvalidModelIdentifier)
            );
        }
    }

    #[test]
    fn only_one_optional_trailing_slash_is_normalized() {
        let without =
            OpenAiCompatibleConfig::new("http://localhost:8000/v1", "model", credential())
                .expect("configuration must be valid");
        let with = OpenAiCompatibleConfig::new("http://localhost:8000/v1/", "model", credential())
            .expect("configuration must be valid");

        assert_eq!(without.base_url.as_str(), "http://localhost:8000/v1");
        assert_eq!(with.base_url.as_str(), "http://localhost:8000/v1");
    }

    #[test]
    fn client_construction_errors_are_sanitized() {
        let secret = "build-error-secret-sentinel\n";
        let config = OpenAiCompatibleConfig::new(
            "http://localhost:8000/v1",
            "model",
            BearerCredential::new(secret).expect("non-empty credential is accepted"),
        )
        .expect("configuration must be valid");

        let error = match build_openai_compatible_model_port(config) {
            Ok(_) => panic!("invalid header credential should not build a client"),
            Err(error) => error,
        };
        let rendered = format!("{error:?} {error}");

        assert_eq!(error, OpenAiCompatibleBuildError::ClientConstructionFailed);
        assert!(!rendered.contains("build-error-secret-sentinel"));
    }
}
