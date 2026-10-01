//! The existence checks a universe hands the orchestrator: the trait, the
//! http(s) HEAD probe, and the `stado://` object probe.

use async_trait::async_trait;

use crate::coverage::{CoverageError, MISSING, PRESENT};
use crate::queue::JobStorage;

/// Returns whether an expected output URI is present (Python `Verifier`).
/// Implementations return [`PRESENT`] or [`MISSING`] and raise on transport
/// errors so the orchestrator fails fast rather than silently retrying.
#[async_trait]
pub trait Verifier: Send + Sync {
    async fn check(&self, expected_uri: &str) -> Result<String, CoverageError>;
}

/// HEAD against an http(s) URI; optional bearer token for HF/private
/// (Python `URIExistsVerifier`). status < 400 -> PRESENT, 404 -> MISSING;
/// anything else, including a 429 rate limit, raises with the status.
pub struct URIExistsVerifier {
    bearer_token: String,
    client: reqwest::Client,
}

impl URIExistsVerifier {
    pub fn new(bearer_token: impl Into<String>) -> Self {
        Self {
            bearer_token: bearer_token.into(),
            client: reqwest::Client::new(),
        }
    }
}

#[async_trait]
impl Verifier for URIExistsVerifier {
    async fn check(&self, expected_uri: &str) -> Result<String, CoverageError> {
        let mut request = self.client.head(expected_uri);
        if !self.bearer_token.is_empty() {
            request = request.header("Authorization", format!("Bearer {}", self.bearer_token));
        }
        let status = request.send().await?.status();
        if status == reqwest::StatusCode::NOT_FOUND {
            return Ok(MISSING.to_string());
        }
        if !status.is_client_error() && !status.is_server_error() {
            return Ok(PRESENT.to_string());
        }
        Err(CoverageError::Other(format!(
            "HEAD {expected_uri}: HTTP {status}"
        )))
    }
}

/// Existence check for a provider-neutral `stado://<namespace>/<key>` object
/// through the backend selected by `STADO_CONFIG`.
pub struct StadoObjectExistsVerifier {
    store: JobStorage,
}

impl StadoObjectExistsVerifier {
    pub fn new(store: JobStorage) -> Self {
        Self { store }
    }

    /// Resolve the configured Stado store without accepting a provider locator.
    pub async fn with_default_store() -> Result<Self, CoverageError> {
        Ok(Self::new(JobStorage::new().await?))
    }
}

#[async_trait]
impl Verifier for StadoObjectExistsVerifier {
    async fn check(&self, expected_uri: &str) -> Result<String, CoverageError> {
        let object = crate::remote::object_store::ObjectRef::parse(expected_uri)
            .map_err(|error| CoverageError::Other(error.to_string()))?;
        let txt = self.store.download_text(&object.storage_path()).await?;
        Ok(if txt.is_some() {
            PRESENT.to_string()
        } else {
            MISSING.to_string()
        })
    }
}
