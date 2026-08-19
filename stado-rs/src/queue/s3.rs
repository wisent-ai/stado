//! Amazon S3 backend using the AWS SDK.
//!
//! Conditional creates and compare-and-swap writes use native `If-None-Match`
//! and `If-Match` support. ETags are opaque backend version tokens. Missing
//! objects and precondition failures are classified from service status;
//! transport and authorization failures remain observable. Listing follows
//! every provider continuation token and retains metadata required by recovery.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use aws_sdk_s3::error::ProvideErrorMetadata;
use aws_sdk_s3::primitives::ByteStream;
use aws_sdk_s3::types::MetadataDirective;
use chrono::{DateTime, Utc};

use super::{gcs::percent_encode, BlobBackend, BlobInfo, StorageError, VersionedText};

struct Inner {
    client: aws_sdk_s3::Client,
    bucket: String,
}

/// S3 implementation of [`BlobBackend`]. Cheap to clone.
#[derive(Clone)]
pub struct S3Backend {
    inner: Arc<Inner>,
}

impl S3Backend {
    /// Build a backend for `bucket` using the AWS adapter host's IMDSv2
    /// identity. Empty `bucket` is the Python RuntimeError
    /// ("WC_S3_BUCKET is required for S3 storage"). Empty `region` lets the SDK
    /// choose its default region.
    pub async fn new(bucket: &str, region: &str) -> Result<Self, StorageError> {
        if bucket.is_empty() {
            return Err(StorageError::Other(
                "WC_S3_BUCKET is required for S3 storage".into(),
            ));
        }
        let shared = crate::providers::aws::sdk_config(region)
            .await
            .map_err(|err| StorageError::Other(err.to_string()))?;
        Ok(Self::assemble(aws_sdk_s3::Client::new(&shared), bucket))
    }

    /// Assemble from an explicit client (tests bind a loopback endpoint).

    fn assemble(client: aws_sdk_s3::Client, bucket: &str) -> Self {
        Self {
            inner: Arc::new(Inner {
                client,
                bucket: bucket.to_string(),
            }),
        }
    }

    /// The bucket this backend is bound to (Python `self.bucket`).
    pub fn bucket(&self) -> &str {
        &self.inner.bucket
    }
}

/// (error code, raw HTTP status) of a failed SDK call, when it was a
/// service (S3-side) error.
fn code_and_status<E: ProvideErrorMetadata>(
    err: &aws_sdk_s3::error::SdkError<E>,
) -> (Option<String>, Option<u16>) {
    match err {
        aws_sdk_s3::error::SdkError::ServiceError(se) => (
            se.err().meta().code().map(str::to_string),
            Some(se.raw().status().as_u16()),
        ),
        _ => (None, None),
    }
}

/// Python's `Code in {"404", "NoSuchKey"}` — both arrive as HTTP 404 (we
/// also accept an explicit NoSuchKey/NotFound code for robustness).
fn is_not_found<E: ProvideErrorMetadata>(err: &aws_sdk_s3::error::SdkError<E>) -> bool {
    let (code, status) = code_and_status(err);
    status == Some(404) || matches!(code.as_deref(), Some("404" | "NoSuchKey" | "NotFound"))
}

/// Python's `Code in {"PreconditionFailed", "412"}` / raw 412.
fn is_precondition_failed<E: ProvideErrorMetadata>(err: &aws_sdk_s3::error::SdkError<E>) -> bool {
    let (code, status) = code_and_status(err);
    status == Some(412) || matches!(code.as_deref(), Some("PreconditionFailed" | "412"))
}

/// Lift an SDK error into [`StorageError::Other`], embedding the S3 error
/// code so operators see "NoSuchKey" / "AccessDenied" etc.
fn sdk_err<E: ProvideErrorMetadata + std::fmt::Debug>(
    op: &str,
    err: aws_sdk_s3::error::SdkError<E>,
) -> StorageError {
    let (code, status) = code_and_status(&err);
    let detail = match (code, status) {
        (Some(code), Some(status)) => format!("{code} (HTTP {status})"),
        (Some(code), None) => code,
        (None, Some(status)) => format!("HTTP {status}"),
        (None, None) => format!("{err}"),
    };
    StorageError::Other(format!("S3 {op} -> {detail}"))
}

/// Strip the surrounding quotes from an S3 ETag (Python `.strip('"')`).
/// S3 ETags always arrive quoted; the CAS version token is unquoted.
fn unquote_etag(etag: &str) -> &str {
    etag.trim_matches('"')
}

/// Convert an SDK timestamp to chrono (nanosecond precision preserved).
fn to_utc(dt: &aws_sdk_s3::primitives::DateTime) -> Option<DateTime<Utc>> {
    DateTime::from_timestamp(dt.secs(), dt.subsec_nanos())
}

/// `CopySource` value for CopyObject: "bucket/key" with the key
/// percent-encoded except `/` separators (boto3's
/// `quote(key, safe='/~')` for the dict form of CopySource).
fn copy_source(bucket: &str, key: &str) -> String {
    let encoded = key
        .split('/')
        .map(percent_encode)
        .collect::<Vec<_>>()
        .join("/");
    format!("{bucket}/{encoded}")
}

#[async_trait]
impl BlobBackend for S3Backend {
    async fn upload_text(&self, path: &str, content: &str) -> Result<(), StorageError> {
        self.upload_bytes(path, content.as_bytes()).await
    }

    async fn upload_bytes(&self, path: &str, content: &[u8]) -> Result<(), StorageError> {
        self.inner
            .client
            .put_object()
            .bucket(&self.inner.bucket)
            .key(path)
            .body(ByteStream::from(content.to_vec()))
            .send()
            .await
            .map_err(|err| sdk_err("put_object", err))?;
        Ok(())
    }

    async fn download_text(&self, path: &str) -> Result<Option<String>, StorageError> {
        let Some(bytes) = self.download_bytes(path).await? else {
            return Ok(None);
        };
        String::from_utf8(bytes)
            .map(Some)
            .map_err(|err| StorageError::Other(format!("invalid UTF-8 in {path}: {err}")))
    }

    async fn download_bytes(&self, path: &str) -> Result<Option<Vec<u8>>, StorageError> {
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

    async fn download_to_filename(&self, path: &str, dest: &Path) -> Result<bool, StorageError> {
        let Some(bytes) = self.download_bytes(path).await? else {
            return Ok(false);
        };
        std::fs::write(dest, bytes)?;
        Ok(true)
    }

    async fn upload_text_if_absent(&self, path: &str, content: &str) -> Result<bool, StorageError> {
        self.upload_bytes_if_absent(path, content.as_bytes().to_vec())
            .await
    }

    async fn upload_file_if_absent(
        &self,
        path: &str,
        local_file: &Path,
    ) -> Result<bool, StorageError> {
        let bytes = std::fs::read(local_file)?;
        self.upload_bytes_if_absent(path, bytes).await
    }

    async fn download_text_versioned(
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

    async fn compare_and_swap_text(
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

    async fn delete(&self, path: &str) -> Result<(), StorageError> {
        self.inner
            .client
            .delete_object()
            .bucket(&self.inner.bucket)
            .key(path)
            .send()
            .await
            .map_err(|err| sdk_err("delete_object", err))?;
        Ok(())
    }

    async fn exists(&self, path: &str) -> Result<bool, StorageError> {
        match self
            .inner
            .client
            .head_object()
            .bucket(&self.inner.bucket)
            .key(path)
            .send()
            .await
        {
            Ok(_) => Ok(true),
            Err(err) if is_not_found(&err) => Ok(false),
            Err(err) => Err(sdk_err("head_object", err)),
        }
    }

    async fn list_paths(
        &self,
        prefix: &str,
        oldest_first: usize,
    ) -> Result<Vec<String>, StorageError> {
        let mut objects = self.list_objects(prefix).await?;
        if oldest_first > 0 {
            // Python: sort by LastModified ascending (None sorts as
            // datetime.min), then take the N oldest.
            objects.sort_by_key(|(_, modified)| *modified);
            objects.truncate(oldest_first);
        }
        Ok(objects.into_iter().map(|(key, _)| key).collect())
    }

    async fn updated_at(&self, path: &str) -> Result<Option<DateTime<Utc>>, StorageError> {
        match self
            .inner
            .client
            .head_object()
            .bucket(&self.inner.bucket)
            .key(path)
            .send()
            .await
        {
            Ok(output) => Ok(output.last_modified().and_then(to_utc)),
            Err(err) if is_not_found(&err) => Ok(None),
            Err(err) => Err(sdk_err("head_object", err)),
        }
    }

    async fn set_metadata(
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

    async fn list_blobs_with_meta(&self, prefix: &str) -> Result<Vec<BlobInfo>, StorageError> {
        let mut out = Vec::new();
        for (key, modified) in self.list_objects(prefix).await? {
            // Python: one head_object per listed key for the metadata map.
            let head = self
                .inner
                .client
                .head_object()
                .bucket(&self.inner.bucket)
                .key(&key)
                .send()
                .await
                .map_err(|err| sdk_err("head_object", err))?;
            out.push(BlobInfo {
                metadata: head
                    .metadata()
                    .map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
                    .unwrap_or_default(),
                name: key,
                updated: modified.and_then(|dt| to_utc(&dt)),
                size: head
                    .content_length()
                    .and_then(|value| u64::try_from(value).ok()),
            });
        }
        Ok(out)
    }
}

impl S3Backend {
    /// PUT with `If-None-Match: *`; `false` on 412 (Python
    /// `_upload_bytes_if_absent`).
    async fn upload_bytes_if_absent(
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

    /// Paginated ListObjectsV2: (key, last_modified) for every object under
    /// `prefix`, in listing order (Python `_objects`).
    async fn list_objects(
        &self,
        prefix: &str,
    ) -> Result<Vec<(String, Option<aws_sdk_s3::primitives::DateTime>)>, StorageError> {
        let mut out = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut request = self
                .inner
                .client
                .list_objects_v2()
                .bucket(&self.inner.bucket)
                .prefix(prefix);
            if let Some(token) = &token {
                request = request.continuation_token(token);
            }
            let page = request
                .send()
                .await
                .map_err(|err| sdk_err("list_objects_v2", err))?;
            for object in page.contents() {
                out.push((
                    object.key().unwrap_or_default().to_string(),
                    object.last_modified().cloned(),
                ));
            }
            match page.next_continuation_token() {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }
        Ok(out)
    }
}
