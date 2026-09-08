//! The existence checks a universe hands the orchestrator: the trait, the
//! http(s) HEAD probe, and the `stado://` object probe.

use async_trait::async_trait;

use crate::config;
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
/// (Python `URIExistsVerifier`). status < 400 -> PRESENT, 404 -> MISSING,
/// 429 -> backoff `COVERAGE_VERIFY_BACKOFF_BASE ** attempt` and retry up to
/// `COVERAGE_HTTP_RETRY_CAP` times; anything else raises.
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
        for attempt in 0..config::COVERAGE_HTTP_RETRY_CAP {
            let mut request = self.client.head(expected_uri);
            if !self.bearer_token.is_empty() {
                request = request.header("Authorization", format!("Bearer {}", self.bearer_token));
            }
            let response = request.send().await?;
            let status = response.status().as_u16();
            if status < 400 {
                return Ok(PRESENT.to_string());
            }
            if status == 404 {
                return Ok(MISSING.to_string());
            }
            if status == 429 {
                let backoff =
                    config::COVERAGE_VERIFY_BACKOFF_BASE.pow(u32::try_from(attempt).unwrap_or(31));
                tokio::time::sleep(std::time::Duration::from_secs(backoff as u64)).await;
                continue;
            }
            // Python re-raises the urllib HTTPError for other statuses.
            return Err(CoverageError::Other(format!(
                "HEAD {expected_uri}: HTTP {status}"
            )));
        }
        Err(format!("HEAD {expected_uri}: retry-cap exceeded").into())
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
        let object = crate::object_store::ObjectRef::parse(expected_uri)
            .map_err(|error| CoverageError::Other(error.to_string()))?;
        let txt = self.store.download_text(&object.storage_path()).await?;
        Ok(if txt.is_some() {
            PRESENT.to_string()
        } else {
            MISSING.to_string()
        })
    }
}
