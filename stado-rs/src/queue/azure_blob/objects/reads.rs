//! The object reads: the body GET, whole or one byte range of it, and the
//! properties HEAD.
//!
//! `objects/mod.rs` carries the trait entries; the routes are here because
//! they share the ETag the versioned read pins, and the three outcomes it has
//! to tell apart — present, absent, and changed under that pin.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use reqwest::{Method, StatusCode};

use crate::queue::StorageError;

use super::super::{
    client::{header_str, parse_http_date},
    AzureBlobBackend,
};

impl AzureBlobBackend {
    /// GET the blob body, or `None` on 404 (Python
    /// `ResourceNotFoundError`).
    pub(super) async fn get_bytes(&self, path: &str, if_match: Option<&str>) -> GetOutcome {
        let headers: Vec<(&str, &str)> = match if_match {
            Some(etag) => vec![("If-Match", etag)],
            None => Vec::new(),
        };
        self.get_with(path, &headers).await
    }

    /// GET bytes `first` through `last` of the blob (`x-ms-range`).
    pub(super) async fn get_range(&self, path: &str, first: u64, last: u64) -> GetOutcome {
        let range = format!("bytes={first}-{last}");
        self.get_with(path, &[("x-ms-range", range.as_str())]).await
    }

    async fn get_with(&self, path: &str, headers: &[(&str, &str)]) -> GetOutcome {
        let response = match self
            .send(Method::GET, &self.blob_url(path), headers, None)
            .await
        {
            Ok(response) => response,
            Err(err) => return GetOutcome::Error(err),
        };
        match response.status() {
            StatusCode::NOT_FOUND => GetOutcome::NotFound,
            StatusCode::PRECONDITION_FAILED => GetOutcome::PreconditionFailed,
            status if status.is_success() => match response.bytes().await {
                Ok(bytes) => GetOutcome::Ok(bytes.to_vec()),
                Err(err) => GetOutcome::Error(err.into()),
            },
            status => GetOutcome::Error(
                Self::api_error(response, &format!("GET {path} ({status})")).await,
            ),
        }
    }

    /// HEAD the blob (Get Blob Properties): (etag, last_modified, metadata)
    /// or `None` on 404. A metadata header whose value is not text is
    /// refused naming it, never left out of the blob's metadata.
    pub(super) async fn head(&self, path: &str) -> Result<Option<BlobProps>, StorageError> {
        let response = self
            .send(Method::HEAD, &self.blob_url(path), &[], None)
            .await?;
        if response.status() == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        let response = Self::ensure_success(response, &format!("HEAD {path}")).await?;
        let etag = header_str(&response, "etag");
        let last_modified = header_str(&response, "last-modified")
            .as_deref()
            .and_then(parse_http_date);
        let mut metadata = BTreeMap::new();
        for (name, value) in response.headers() {
            let Some(key) = name.as_str().strip_prefix("x-ms-meta-") else {
                continue;
            };
            let value = value.to_str().map_err(|error| {
                StorageError::Other(format!(
                    "HEAD {path} answered metadata {key} that is not text: {error}"
                ))
            })?;
            metadata.insert(key.to_string(), value.to_string());
        }
        let size = match response.headers().get("content-length") {
            None => None,
            Some(value) => {
                let text = value.to_str().map_err(|error| {
                    StorageError::Other(format!(
                        "HEAD {path} answered a content length that is not text: {error}"
                    ))
                })?;
                Some(text.parse::<u64>().map_err(|error| {
                    StorageError::Other(format!(
                        "HEAD {path} answered a content length {text:?} that is no byte count: {error}"
                    ))
                })?)
            }
        };
        Ok(Some(BlobProps {
            etag,
            last_modified,
            size,
            metadata,
        }))
    }
}

/// Outcome of a pinned/unpinned GET, so the versioned-read retry loop can
/// distinguish the three Python exception branches.
pub(super) enum GetOutcome {
    Ok(Vec<u8>),
    NotFound,
    PreconditionFailed,
    Error(StorageError),
}

/// Result of Get Blob Properties.
pub(super) struct BlobProps {
    pub(super) etag: Option<String>,
    pub(super) last_modified: Option<DateTime<Utc>>,
    /// The blob's length in bytes, from Content-Length.
    pub(super) size: Option<u64>,
    pub(super) metadata: BTreeMap<String, String>,
}
