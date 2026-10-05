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
/// (Python `URIExistsVerifier`). status < 400 -> PRESENT, 404 -> MISSING. A
/// 429 that states its `Retry-After` is waited out and asked again, so the
/// walk runs as wide as the machine and the server sets the pace; a 429 that
/// states nothing, and any other failure, raises with the status.
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

/// How long a rate-limited answer asks the client to wait: `Retry-After` as
/// seconds or as an HTTP date. `None` when the answer states neither.
fn retry_after(headers: &reqwest::header::HeaderMap) -> Option<std::time::Duration> {
    let value = headers
        .get(reqwest::header::RETRY_AFTER)?
        .to_str()
        .ok()?
        .trim();
    if let Ok(seconds) = value.parse::<u64>() {
        return Some(std::time::Duration::from_secs(seconds));
    }
    let at = chrono::DateTime::parse_from_rfc2822(value).ok()?;
    (at.with_timezone(&chrono::Utc) - chrono::Utc::now())
        .to_std()
        .ok()
        .or(Some(std::time::Duration::ZERO))
}

#[async_trait]
impl Verifier for URIExistsVerifier {
    async fn check(&self, expected_uri: &str) -> Result<String, CoverageError> {
        loop {
            let mut request = self.client.head(expected_uri);
            if !self.bearer_token.is_empty() {
                request = request.header("Authorization", format!("Bearer {}", self.bearer_token));
            }
            let response = request.send().await?;
            let status = response.status();
            if status == reqwest::StatusCode::NOT_FOUND {
                return Ok(MISSING.to_string());
            }
            if !status.is_client_error() && !status.is_server_error() {
                return Ok(PRESENT.to_string());
            }
            if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
                if let Some(wait) = retry_after(response.headers()) {
                    tokio::time::sleep(wait).await;
                    continue;
                }
            }
            return Err(CoverageError::Other(format!(
                "HEAD {expected_uri}: HTTP {status}"
            )));
        }
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
