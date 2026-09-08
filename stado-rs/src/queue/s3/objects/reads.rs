//! The object reads: the GetObject that answers with a body, and the one
//! that answers with a body and the ETag identifying it.
//!
//! `objects/mod.rs` carries the trait entries; the pair is here because both
//! are the same GetObject round-trip reading 404 as absence rather than as a
//! failure, and the versioned one only adds the ETag the caller will swap
//! against.

use crate::queue::{StorageError, VersionedText};

use super::super::{
    client::unquote_etag,
    refusals::{is_not_found, sdk_err},
    S3Backend,
};

impl S3Backend {
    /// GET the object body, or `None` when the key does not exist.
    pub(super) async fn get_object_bytes(
        &self,
        path: &str,
    ) -> Result<Option<Vec<u8>>, StorageError> {
        let output = match self
            .inner
            .client
            .get_object()
            .bucket(&self.inner.bucket)
            .key(path)
            .send()
            .await
        {
            Ok(output) => output,
            Err(err) if is_not_found(&err) => return Ok(None),
            Err(err) => return Err(sdk_err("get_object", err)),
        };
        let bytes = output
            .body
            .collect()
            .await
            .map_err(|err| StorageError::Other(format!("S3 get_object body -> {err}")))?
            .into_bytes();
        Ok(Some(bytes.to_vec()))
    }

    /// GET the object body together with the ETag it was served at, the
    /// token a later compare-and-swap sends back.
    pub(super) async fn download_versioned_text(
        &self,
        path: &str,
    ) -> Result<Option<VersionedText>, StorageError> {
        let output = match self
            .inner
            .client
            .get_object()
            .bucket(&self.inner.bucket)
            .key(path)
            .send()
            .await
        {
            Ok(output) => output,
            Err(err) if is_not_found(&err) => return Ok(None),
            Err(err) => return Err(sdk_err("get_object", err)),
        };
        // Python `response["ETag"].strip('"')` — the version token is the
        // UNQUOTED ETag.
        let version = output
            .e_tag()
            .map(unquote_etag)
            .unwrap_or_default()
            .to_string();
        let bytes = output
            .body
            .collect()
            .await
            .map_err(|err| StorageError::Other(format!("S3 get_object body -> {err}")))?
            .into_bytes();
        let content = String::from_utf8(bytes.to_vec())
            .map_err(|err| StorageError::Other(format!("invalid UTF-8 in {path}: {err}")))?;
        Ok(Some(VersionedText { content, version }))
    }
}
