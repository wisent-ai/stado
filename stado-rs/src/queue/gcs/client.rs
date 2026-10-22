//! The constructor, the scoped bearer token, the authenticated sender, and
//! the one timestamp read back off a response.
//!
//! Every request the sibling components make leaves through `send`, so the
//! token provider is consulted and the `Authorization` header attached in
//! exactly one place; the object resource fields that carry a time are
//! decoded here in that same wire vocabulary.

use std::sync::Arc;

use chrono::{DateTime, Utc};
use reqwest::Method;

use crate::queue::StorageError;

use super::{GcsBackend, Inner, STORAGE_SCOPE};

impl GcsBackend {
    /// Build a backend for `bucket`; authentication failure is terminal.
    pub async fn new(bucket: &str) -> Result<Self, StorageError> {
        let auth = crate::skarbiec::gcp_provider().await.map_err(|err| {
            StorageError::Auth(format!(
                "no scoped GCP credentials found for the GCS backend: {err}"
            ))
        })?;
        Ok(Self {
            inner: Arc::new(Inner {
                client: reqwest::Client::new(),
                bucket: bucket.to_string(),
                auth,
            }),
        })
    }

    /// The bucket this backend is bound to (`config::bucket()` by default).
    pub fn bucket(&self) -> &str {
        &self.inner.bucket
    }

    /// Fresh (cached by gcp_auth until expiry) bearer token.
    async fn token(&self) -> Result<String, StorageError> {
        let token = self
            .inner
            .auth
            .token(&[STORAGE_SCOPE])
            .await
            .map_err(|err| StorageError::Auth(err.to_string()))?;
        Ok(format!("Bearer {}", token.as_str()))
    }

    /// Execute an authenticated request and pass through success responses.
    pub(super) async fn send(
        &self,
        method: Method,
        url: &str,
        body: Option<(String, Vec<u8>)>,
    ) -> Result<reqwest::Response, StorageError> {
        let mut request = self
            .inner
            .client
            .request(method, url)
            .header(reqwest::header::AUTHORIZATION, self.token().await?);
        if let Some((content_type, bytes)) = body {
            request = request
                .header(reqwest::header::CONTENT_TYPE, content_type)
                .body(bytes);
        }
        Ok(crate::wait::request(request).await?)
    }

    /// Execute an authenticated GET of bytes `first` through `last` of the
    /// media at `url`.
    pub(super) async fn send_range(
        &self,
        url: &str,
        first: u64,
        last: u64,
    ) -> Result<reqwest::Response, StorageError> {
        let request = self
            .inner
            .client
            .request(Method::GET, url)
            .header(reqwest::header::AUTHORIZATION, self.token().await?)
            .header(reqwest::header::RANGE, format!("bytes={first}-{last}"));
        Ok(crate::wait::request(request).await?)
    }
}

/// The RFC 3339 time a GCS object resource states in `value`, normalized to
/// UTC; `None` when the resource states none. A value that is not one is
/// refused naming `what` it was read for, never read as no time.
pub(super) fn parse_timestamp(
    value: Option<&serde_json::Value>,
    what: &str,
) -> Result<Option<DateTime<Utc>>, StorageError> {
    let Some(value) = value else {
        return Ok(None);
    };
    let raw = value.as_str().ok_or_else(|| {
        StorageError::Other(format!(
            "GCS states the updated time of {what} as {value}, not a string"
        ))
    })?;
    DateTime::parse_from_rfc3339(raw)
        .map(|at| Some(at.with_timezone(&Utc)))
        .map_err(|error| {
            StorageError::Other(format!(
                "GCS states the updated time of {what} as {raw:?}, not an RFC 3339 time: {error}"
            ))
        })
}
