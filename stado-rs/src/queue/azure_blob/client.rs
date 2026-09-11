//! The constructor, the authenticated sender, and the wire values read back
//! off a response.
//!
//! [`AzureBlobBackend::new`] refuses an empty account or container before
//! anything is sent, and every request the sibling components make goes out
//! through `send`, so the pinned API version, the RFC1123 date and the token
//! chain are attached in exactly one place. The two readers below decode what
//! the service answers with in that same wire vocabulary.

use std::sync::Arc;

use chrono::{DateTime, Utc};
use reqwest::Method;

use crate::queue::StorageError;

use super::{AzureBlobBackend, Inner, STORAGE_RESOURCE, STORAGE_SCOPE, X_MS_VERSION};

impl AzureBlobBackend {
    /// Build a backend for `account`/`container`. Empty values are the
    /// Python RuntimeErrors, verbatim.
    pub fn new(account: &str, container: &str) -> Result<Self, StorageError> {
        if account.is_empty() {
            return Err(StorageError::Other(
                "WC_AZURE_STORAGE_ACCOUNT env var is empty; cannot construct AzureBlobBackend"
                    .into(),
            ));
        }
        if container.is_empty() {
            return Err(StorageError::Other(
                "WC_AZURE_CONTAINER env var is empty; cannot construct AzureBlobBackend".into(),
            ));
        }
        Ok(Self::assemble(
            &format!("https://{account}.blob.core.windows.net"),
            account,
            container,
            true,
        ))
    }

    /// Bind to an explicit base URL without auth (loopback mocks in tests).
    fn assemble(base_url: &str, account: &str, container: &str, auth: bool) -> Self {
        Self {
            inner: Arc::new(Inner {
                http: reqwest::Client::new(),
                account: account.to_string(),
                container: container.to_string(),
                base_url: base_url.trim_end_matches('/').to_string(),
                auth,
            }),
        }
    }

    /// The storage account (Python `self.account`).
    pub fn account(&self) -> &str {
        &self.inner.account
    }

    /// The container (bucket-equivalent; Python `self.container_name`).
    pub fn container(&self) -> &str {
        &self.inner.container
    }

    /// Send one authenticated request with the pinned API version. Extra
    /// headers are caller-supplied (conditionals, blob type, metadata).
    pub(super) async fn send(
        &self,
        method: Method,
        url: &str,
        headers: &[(&str, &str)],
        body: Option<Vec<u8>>,
    ) -> Result<reqwest::Response, StorageError> {
        let mut request = self
            .inner
            .http
            .request(method, url)
            .header("x-ms-version", X_MS_VERSION)
            // RFC1123 IMF-fixdate, e.g. "Sun, 26 Jul 2026 03:44:52 GMT".
            .header(
                "x-ms-date",
                Utc::now().format("%a, %d %b %Y %H:%M:%S GMT").to_string(),
            );
        if self.inner.auth {
            let token = crate::remote::azure_token::bearer_token(
                &self.inner.http,
                STORAGE_SCOPE,
                STORAGE_RESOURCE,
            )
            .await
            .map_err(|err| match err {
                crate::remote::azure_token::TokenError::Auth(msg) => StorageError::Auth(msg),
                crate::remote::azure_token::TokenError::Http(err) => StorageError::Http(err),
            })?;
            request = request.header(reqwest::header::AUTHORIZATION, format!("Bearer {token}"));
        }
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        if let Some(body) = body {
            request = request.body(body);
        }
        Ok(request.send().await?)
    }
}

/// Read a response header as an owned string.
pub(super) fn header_str(response: &reqwest::Response, name: &str) -> Option<String> {
    response
        .headers()
        .get(name)?
        .to_str()
        .ok()
        .map(str::to_string)
}

/// Parse an RFC1123 HTTP date ("Fri, 02 Jan 2026 03:04:05 GMT") — the
/// format of Last-Modified / Creation-Time in headers and list XML.
pub(super) fn parse_http_date(raw: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc2822(raw)
        .ok()
        .map(|dt| dt.with_timezone(&Utc))
}
