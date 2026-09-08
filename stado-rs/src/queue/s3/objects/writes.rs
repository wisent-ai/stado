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
    /// `_upload_bytes_if_absent`).
    pub(super) async fn upload_bytes_if_absent(
        &self,
        path: &str,
        bytes: Vec<u8>,
    ) -> Result<bool, StorageError> {
        match self
            .inner
            .client
            .put_object()
            .bucket(&self.inner.bucket)
            .key(path)
            .if_none_match("*")
            .body(ByteStream::from(bytes))
            .send()
            .await
        {
            Ok(_) => Ok(true),
            Err(err) if is_precondition_failed(&err) => Ok(false),
            Err(err) => Err(sdk_err("put_object if-absent", err)),
        }
    }

    /// PUT with `If-Match: "<expected_version>"`; 412 is the lost race, and
    /// the ETag of the accepted write is the next version token.
    pub(super) async fn compare_and_swap_object(
        &self,
        path: &str,
        expected_version: &str,
        content: &str,
    ) -> Result<String, StorageError> {
        // Python sends If-Match: f'"{expected_etag}"' — the token is stored
        // unquoted and re-quoted for the wire.
        let output = self
            .inner
            .client
            .put_object()
            .bucket(&self.inner.bucket)
            .key(path)
            .if_match(format!("\"{expected_version}\""))
            .body(ByteStream::from(content.as_bytes().to_vec()))
            .send()
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
        // Python: response.headers.get("etag", "").strip('"') — possibly "".
        Ok(output
            .e_tag()
            .map(unquote_etag)
            .unwrap_or_default()
            .to_string())
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
        let head = self
            .inner
            .client
            .head_object()
            .bucket(&self.inner.bucket)
            .key(path)
            .send()
            .await
            .map_err(|err| sdk_err("head_object", err))?;
        let mut metadata: BTreeMap<String, String> = head
            .metadata()
            .map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
            .unwrap_or_default();
        metadata.extend(
            kv.iter()
                .filter(|(_, v)| !v.is_empty())
                .map(|(k, v)| (k.clone(), v.clone())),
        );
        let content_type = head
            .content_type()
            .unwrap_or("application/octet-stream")
            .to_string();
        let mut request = self
            .inner
            .client
            .copy_object()
            .bucket(&self.inner.bucket)
            .key(path)
            .copy_source(copy_source(&self.inner.bucket, path))
            .metadata_directive(MetadataDirective::Replace)
            .content_type(content_type);
        for (key, value) in &metadata {
            request = request.metadata(key, value);
        }
        request
            .send()
            .await
            .map_err(|err| sdk_err("copy_object", err))?;
        Ok(())
    }
}
