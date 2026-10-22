//! The object writes that carry a precondition or a second round-trip: the
//! if-absent create, the compare-and-swap, and the metadata copy-in-place.
//!
//! `objects/mod.rs` carries the trait entries; the plain PutObject stays
//! there because it is one request with no header to reason about, while
//! these three each owe the caller a classified answer — 412 as a lost race,
//! and the head-then-copy the metadata merge is spelled as.

use std::collections::BTreeMap;

use aws_sdk_s3::primitives::ByteStream;
use aws_sdk_s3::types::MetadataDirective;

use crate::queue::StorageError;

use super::super::{
    client::unquote_etag,
    refusals::{is_precondition_failed, sdk_err},
    uri::copy_source,
    S3Backend,
};

impl S3Backend {
    /// PUT with `If-None-Match: *`; `false` on 412 (Python
    /// `_upload_bytes_if_absent`). Refused unless the store honours the
    /// precondition (`probe`).
    pub(super) async fn upload_bytes_if_absent(
        &self,
        path: &str,
        bytes: Vec<u8>,
    ) -> Result<bool, StorageError> {
        self.require_conditional_writes().await?;
        match crate::wait::sdk(
            self.inner
                .client
                .put_object()
                .bucket(&self.inner.bucket)
                .key(path)
                .if_none_match("*")
                .body(ByteStream::from(bytes))
                .send(),
        )
        .await
        {
            Ok(_) => Ok(true),
            Err(err) if is_precondition_failed(&err) => Ok(false),
            Err(err) => Err(sdk_err("put_object if-absent", err)),
        }
    }

    /// PUT with `If-Match: "<expected_version>"`; 412 is the lost race, and
    /// the ETag of the accepted write is the next version token. Refused
    /// unless the store honours the precondition (`probe`).
    pub(super) async fn compare_and_swap_object(
        &self,
        path: &str,
        expected_version: &str,
        content: &str,
    ) -> Result<String, StorageError> {
        self.require_conditional_writes().await?;
        // Python sends If-Match: f'"{expected_etag}"' — the token is stored
        // unquoted and re-quoted for the wire.
        let output = crate::wait::sdk(
            self.inner
                .client
                .put_object()
                .bucket(&self.inner.bucket)
                .key(path)
                .if_match(format!("\"{expected_version}\""))
                .body(ByteStream::from(content.as_bytes().to_vec()))
                .send(),
        )
        .await;
        let output = match output {
            Ok(output) => output,
            Err(err) if is_precondition_failed(&err) => {
                return Err(StorageError::StorageConflict(format!(
                    "{path} changed concurrently"
                )));
            }
            Err(err) => return Err(sdk_err("conditional put_object", err)),
        };
        // The ETag is the next version token; a store that answers none
        // leaves the next compare-and-swap nothing to name.
        match output.e_tag().map(unquote_etag) {
            Some(version) if !version.is_empty() => Ok(version.to_string()),
            _ => Err(StorageError::Other(format!(
                "the conditional put_object of {path} was stored, but the store answered no \
                 ETag, so the object has no version a later compare-and-swap could name"
            ))),
        }
    }

    /// Merge `kv` into the object's metadata by copying it onto itself with
    /// the merged map, the only way S3 rewrites metadata in place.
    pub(super) async fn copy_object_with_metadata(
        &self,
        path: &str,
        kv: &BTreeMap<String, String>,
    ) -> Result<(), StorageError> {
        // Python: head_object (propagates when the object is missing),
        // merge skipping empty values, copy-in-place with REPLACE.
        let head = crate::wait::sdk(
            self.inner
                .client
                .head_object()
                .bucket(&self.inner.bucket)
                .key(path)
                .send(),
        )
        .await
        .map_err(|err| sdk_err("head_object", err))?;
        // An object without user metadata carries no metadata map.
        let mut metadata = BTreeMap::new();
        if let Some(held) = head.metadata() {
            metadata.extend(held.iter().map(|(k, v)| (k.clone(), v.clone())));
        }
        metadata.extend(
            kv.iter()
                .filter(|(_, v)| !v.is_empty())
                .map(|(k, v)| (k.clone(), v.clone())),
        );
        let mut request = self
            .inner
            .client
            .copy_object()
            .bucket(&self.inner.bucket)
            .key(path)
            .copy_source(copy_source(&self.inner.bucket, path))
            .metadata_directive(MetadataDirective::Replace);
        // The content type the object carries is kept; one it never had is
        // not invented.
        if let Some(content_type) = head.content_type() {
            request = request.content_type(content_type);
        }
        for (key, value) in &metadata {
            request = request.metadata(key, value);
        }
        crate::wait::sdk(request.send())
            .await
            .map_err(|err| sdk_err("copy_object", err))?;
        Ok(())
    }
}
