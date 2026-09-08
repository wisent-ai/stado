//! The object reads: the body GET and the properties HEAD.
//!
//! `objects/mod.rs` carries the trait entries; both routes are here because
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
        let response = match self
            .send(Method::GET, &self.blob_url(path), &headers, None)
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
            _ => GetOutcome::Error(Self::api_error(response, &format!("GET {path}")).await),
        }
    }

    /// HEAD the blob (Get Blob Properties): (etag, last_modified, metadata)
    /// or `None` on 404.
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
        let metadata = response
            .headers()
            .iter()
            .filter_map(|(name, value)| {
                let key = name.as_str().strip_prefix("x-ms-meta-")?;
                Some((key.to_string(), value.to_str().ok()?.to_string()))
            })
            .collect();
        Ok(Some(BlobProps {
            etag,
            last_modified,
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
    pub(super) metadata: BTreeMap<String, String>,
}
