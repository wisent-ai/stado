//! The object writes: Put Blob, and the conditional header that makes one
//! of them a create rather than an overwrite.
//!
//! `objects/mod.rs` carries the trait entries; the conditional is here
//! because the unconditional write, the if-absent create and the CAS are the
//! same request with one header changed.

use reqwest::{Method, StatusCode};

use crate::queue::StorageError;

use super::super::AzureBlobBackend;

impl AzureBlobBackend {
    /// Put Blob; `conditional` is `If-None-Match: *` (if-absent) or
    /// `If-Match: etag` (CAS). Returns the raw response for status mapping.
    pub(super) async fn put_blob(
        &self,
        path: &str,
        bytes: Vec<u8>,
        conditional: Option<(&str, &str)>,
    ) -> Result<reqwest::Response, StorageError> {
        let mut headers: Vec<(&str, &str)> = vec![("x-ms-blob-type", "BlockBlob")];
        if let Some((name, value)) = conditional {
            headers.push((name, value));
        }
        self.send(Method::PUT, &self.blob_url(path), &headers, Some(bytes))
            .await
    }

    /// PUT with `If-None-Match: *`; `false` on 409 (Python
    /// `ResourceExistsError`).
    pub(super) async fn upload_bytes_if_absent(
        &self,
        path: &str,
        bytes: Vec<u8>,
    ) -> Result<bool, StorageError> {
        let response = self
            .put_blob(path, bytes, Some(("If-None-Match", "*")))
            .await?;
        if response.status() == StatusCode::CONFLICT {
            return Ok(false);
        }
        Self::ensure_success(response, &format!("PUT {path} if-absent")).await?;
        Ok(true)
    }
}
