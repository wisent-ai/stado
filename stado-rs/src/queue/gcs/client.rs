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
        Ok(request.send().await?)
    }
}

/// Parse an RFC3339 GCS timestamp ("2026-05-16T12:34:56.789Z").
pub(super) fn parse_timestamp(value: &serde_json::Value) -> Option<DateTime<Utc>> {
    let raw = value.as_str()?;
    DateTime::parse_from_rfc3339(raw)
        .ok()
        .map(|dt| dt.with_timezone(&Utc))
}
