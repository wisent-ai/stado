//! The object reads: the GetObject that answers with a body, whole or one
//! byte range of it, and the one that answers with a body and the ETag
//! identifying it.
//!
//! `objects/mod.rs` carries the trait entries; the reads are here because
//! all are the same GetObject round-trip reading 404 as absence rather than
//! as a failure, and the versioned one only adds the ETag the caller will
//! swap against.

use aws_sdk_s3::operation::get_object::GetObjectOutput;

use crate::queue::{StorageError, VersionedText};

use super::super::{
    client::unquote_etag,
    refusals::{is_not_found, sdk_err},
    S3Backend,
};

impl S3Backend {
    /// GET the object, the byte range `range` names (`bytes=<first>-<last>`)
    /// when one is given, or `None` when the key does not exist.
    async fn get_object(
        &self,
        path: &str,
        range: Option<String>,
    ) -> Result<Option<GetObjectOutput>, StorageError> {
        let request = self
            .inner
            .client
            .get_object()
            .bucket(&self.inner.bucket)
            .key(path)
            .set_range(range);
        match crate::wait::sdk(request.send()).await {
            Ok(output) => Ok(Some(output)),
            Err(err) if is_not_found(&err) => Ok(None),
            Err(err) => Err(sdk_err("get_object", err)),
        }
    }

    /// The body a GetObject answered.
    async fn body(output: GetObjectOutput) -> Result<Vec<u8>, StorageError> {
        let bytes = output
            .body
            .collect()
            .await
            .map_err(|err| StorageError::Other(format!("S3 get_object body -> {err}")))?
            .into_bytes();
        Ok(bytes.to_vec())
    }

    /// GET the object body, or `None` when the key does not exist.
    pub(super) async fn get_object_bytes(
        &self,
        path: &str,
    ) -> Result<Option<Vec<u8>>, StorageError> {
        match self.get_object(path, None).await? {
            Some(output) => Ok(Some(Self::body(output).await?)),
            None => Ok(None),
        }
    }

    /// GET bytes `first` through `last` of the object, or `None` when the
    /// key does not exist.
    pub(super) async fn get_object_range(
        &self,
        path: &str,
        first: u64,
        last: u64,
    ) -> Result<Option<Vec<u8>>, StorageError> {
        let range = Some(format!("bytes={first}-{last}"));
        match self.get_object(path, range).await? {
            Some(output) => Ok(Some(Self::body(output).await?)),
            None => Ok(None),
        }
    }

    /// GET the object body together with the ETag it was served at, the
    /// token a later compare-and-swap sends back. A store that answers no
    /// ETag leaves that swap nothing to name, so the read is refused.
    pub(super) async fn download_versioned_text(
        &self,
        path: &str,
    ) -> Result<Option<VersionedText>, StorageError> {
        let Some(output) = self.get_object(path, None).await? else {
            return Ok(None);
        };
        // Python `response["ETag"].strip('"')` — the version token is the
        // UNQUOTED ETag.
        let version = match output.e_tag().map(unquote_etag) {
            Some(version) if !version.is_empty() => version.to_string(),
            Some(_) | None => {
                return Err(StorageError::Other(format!(
                    "S3 GetObject of {path} answered no ETag, so no compare-and-swap of it \
                     could name the version it read"
                )))
            }
        };
        let bytes = Self::body(output).await?;
        let content = String::from_utf8(bytes)
            .map_err(|err| StorageError::Other(format!("invalid UTF-8 in {path}: {err}")))?;
        Ok(Some(VersionedText { content, version }))
    }
}
