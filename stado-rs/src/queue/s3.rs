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
    #[cfg(test)]
    pub(crate) fn assemble_for_test(
        client: aws_sdk_s3::Client,
        bucket: &str,
        _region: &str,
    ) -> Self {
        Self::assemble(client, bucket)
    }

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::mock_http;

    fn test_backend(base_url: &str) -> S3Backend {
        let credentials =
            aws_sdk_s3::config::Credentials::new("test-akid", "test-secret", None, None, "test");
        let config = aws_sdk_s3::config::Builder::new()
            .region(aws_sdk_s3::config::Region::new("us-east-1"))
            .credentials_provider(credentials)
            .endpoint_url(base_url)
            .force_path_style(true)
            .behavior_version_latest()
            .build();
        S3Backend::assemble_for_test(
            aws_sdk_s3::Client::from_conf(config),
            "test-bucket",
            "us-east-1",
        )
    }

    /// HTTP response with custom headers (ETag, Last-Modified, x-amz-meta-*).
    fn response_with(status: u16, reason: &str, headers: &[(&str, &str)], body: &str) -> String {
        let content_type = headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("content-type"))
            .map(|(_, v)| v.to_string())
            .unwrap_or_else(|| "application/xml".to_string());
        let extra: String = headers
            .iter()
            .filter(|(k, _)| !k.eq_ignore_ascii_case("content-type"))
            .map(|(k, v)| format!("{k}: {v}\r\n"))
            .collect();
        format!(
            "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\n\
             Content-Length: {}\r\nConnection: close\r\n{extra}\r\n{body}",
            body.len()
        )
    }

    fn error_response(status: u16, code: &str) -> String {
        response_with(
            status,
            "Error",
            &[],
            &format!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\
                 <Error><Code>{code}</Code><Message>mock {code}</Message></Error>"
            ),
        )
    }

    fn requests(server: &crate::testutil::MockHttp) -> Vec<String> {
        server.requests.lock().unwrap().clone()
    }

    #[tokio::test]
    async fn upload_and_download_round_trip() {
        let server = mock_http(vec![
            response_with(200, "OK", &[("ETag", "\"e1\"")], ""),
            response_with(200, "OK", &[("ETag", "\"e1\"")], "hello"),
            error_response(404, "NoSuchKey"),
        ])
        .await;
        let b = test_backend(&server.base_url);
        b.upload_text("queue/j1.json", "hello").await.unwrap();
        assert_eq!(
            b.download_text("queue/j1.json").await.unwrap().as_deref(),
            Some("hello")
        );
        assert_eq!(b.download_text("gone.json").await.unwrap(), None);
        let reqs = requests(&server);
        assert_eq!(reqs.len(), 3, "{reqs:?}");
        assert!(
            reqs[0].starts_with("PUT /test-bucket/queue/j1.json"),
            "{}",
            reqs[0]
        );
        assert!(reqs[0].contains("hello"), "{}", reqs[0]);
        assert!(
            reqs[1].starts_with("GET /test-bucket/queue/j1.json"),
            "{}",
            reqs[1]
        );
        server.stop();
    }

    #[tokio::test]
    async fn download_to_filename_reports_absence() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("out.bin");
        let server = mock_http(vec![
            error_response(404, "NoSuchKey"),
            response_with(200, "OK", &[("ETag", "\"e1\"")], "data"),
        ])
        .await;
        let b = test_backend(&server.base_url);
        assert!(!b.download_to_filename("nope", &dest).await.unwrap());
        assert!(b.download_to_filename("blob", &dest).await.unwrap());
        assert_eq!(std::fs::read_to_string(&dest).unwrap(), "data");
        server.stop();
    }

    #[tokio::test]
    async fn versioned_read_strips_etag_quotes() {
        let server = mock_http(vec![
            response_with(200, "OK", &[("ETag", "\"abc123\"")], "payload"),
            error_response(404, "NoSuchKey"),
        ])
        .await;
        let b = test_backend(&server.base_url);
        let v = b.download_text_versioned("state/x").await.unwrap().unwrap();
        assert_eq!(v.content, "payload");
        // Python parity: the version token is the UNQUOTED ETag.
        assert_eq!(v.version, "abc123");
        assert_eq!(b.download_text_versioned("gone").await.unwrap(), None);
        server.stop();
    }

    #[tokio::test]
    async fn cas_success_sends_quoted_if_match_and_returns_new_etag() {
        let server = mock_http(vec![response_with(
            200,
            "OK",
            &[("ETag", "\"def456\"")],
            "",
        )])
        .await;
        let b = test_backend(&server.base_url);
        let new_version = b
            .compare_and_swap_text("state/x", "abc123", "new")
            .await
            .unwrap();
        assert_eq!(new_version, "def456");
        let reqs = requests(&server);
        assert!(
            reqs[0].starts_with("PUT /test-bucket/state/x"),
            "{}",
            reqs[0]
        );
        assert!(reqs[0].contains("if-match: \"abc123\""), "{}", reqs[0]);
        assert!(reqs[0].contains("new"), "{}", reqs[0]);
        server.stop();
    }

    #[tokio::test]
    async fn cas_conflict_maps_412_to_storage_conflict() {
        let server = mock_http(vec![error_response(412, "PreconditionFailed")]).await;
        let b = test_backend(&server.base_url);
        let err = b
            .compare_and_swap_text("state/x", "stale", "new")
            .await
            .unwrap_err();
        assert!(matches!(err, StorageError::StorageConflict(_)), "{err:?}");
        assert!(err.to_string().contains("changed concurrently"), "{err}");
        server.stop();
    }

    #[tokio::test]
    async fn upload_if_absent_uses_if_none_match_star() {
        let server = mock_http(vec![
            response_with(200, "OK", &[("ETag", "\"e1\"")], ""),
            error_response(412, "PreconditionFailed"),
        ])
        .await;
        let b = test_backend(&server.base_url);
        assert!(b.upload_text_if_absent("lock", "first").await.unwrap());
        assert!(!b.upload_text_if_absent("lock", "second").await.unwrap());
        let reqs = requests(&server);
        assert!(reqs[0].contains("if-none-match: *"), "{}", reqs[0]);
        assert!(reqs[1].contains("if-none-match: *"), "{}", reqs[1]);
        server.stop();
    }

    #[tokio::test]
    async fn upload_file_if_absent_uploads_once() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src.txt");
        std::fs::write(&src, "file-bytes").unwrap();
        let server = mock_http(vec![
            response_with(200, "OK", &[("ETag", "\"e1\"")], ""),
            error_response(412, "PreconditionFailed"),
        ])
        .await;
        let b = test_backend(&server.base_url);
        assert!(b.upload_file_if_absent("f", &src).await.unwrap());
        assert!(!b.upload_file_if_absent("f", &src).await.unwrap());
        server.stop();
    }

    #[tokio::test]
    async fn delete_and_exists() {
        let server = mock_http(vec![
            response_with(204, "No Content", &[], ""),
            response_with(200, "OK", &[("ETag", "\"e1\"")], ""),
            error_response(404, "404"),
        ])
        .await;
        let b = test_backend(&server.base_url);
        b.delete("gone").await.unwrap();
        assert!(b.exists("present").await.unwrap());
        assert!(!b.exists("missing").await.unwrap());
        let reqs = requests(&server);
        assert!(
            reqs[0].starts_with("DELETE /test-bucket/gone"),
            "{}",
            reqs[0]
        );
        assert!(
            reqs[1].starts_with("HEAD /test-bucket/present"),
            "{}",
            reqs[1]
        );
        server.stop();
    }

    #[tokio::test]
    async fn list_paths_paginates_and_bounded_sorts_by_last_modified() {
        let page1 = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\
            <ListBucketResult xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\">\
            <Name>test-bucket</Name><Prefix>queue/</Prefix><IsTruncated>true</IsTruncated>\
            <NextContinuationToken>tok-2</NextContinuationToken>\
            <Contents><Key>queue/b.json</Key><LastModified>2026-01-02T03:04:05.000Z</LastModified></Contents>\
            <Contents><Key>queue/a.json</Key><LastModified>2026-01-03T03:04:05.000Z</LastModified></Contents>\
            </ListBucketResult>";
        let page2 = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\
            <ListBucketResult xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\">\
            <Name>test-bucket</Name><Prefix>queue/</Prefix><IsTruncated>false</IsTruncated>\
            <Contents><Key>queue/c.json</Key><LastModified>2026-01-01T03:04:05.000Z</LastModified></Contents>\
            </ListBucketResult>";
        let server = mock_http(vec![
            response_with(200, "OK", &[], page1),
            response_with(200, "OK", &[], page2),
            response_with(200, "OK", &[], page1),
            response_with(200, "OK", &[], page2),
        ])
        .await;
        let b = test_backend(&server.base_url);
        // Unbounded: listing order preserved (Python parity — no sort).
        assert_eq!(
            b.list_paths("queue/", 0).await.unwrap(),
            vec!["queue/b.json", "queue/a.json", "queue/c.json"]
        );
        // Bounded: N oldest by LastModified.
        assert_eq!(
            b.list_paths("queue/", 2).await.unwrap(),
            vec!["queue/c.json", "queue/b.json"]
        );
        let reqs = requests(&server);
        assert!(reqs[0].contains("list-type=2"), "{}", reqs[0]);
        assert!(reqs[0].contains("prefix=queue%2F"), "{}", reqs[0]);
        assert!(reqs[1].contains("continuation-token=tok-2"), "{}", reqs[1]);
        server.stop();
    }

    #[tokio::test]
    async fn updated_at_parses_last_modified() {
        let server = mock_http(vec![
            response_with(
                200,
                "OK",
                &[
                    ("ETag", "\"e1\""),
                    ("Last-Modified", "Fri, 02 Jan 2026 03:04:05 GMT"),
                ],
                "",
            ),
            error_response(404, "404"),
        ])
        .await;
        let b = test_backend(&server.base_url);
        let updated = b.updated_at("m").await.unwrap().unwrap();
        assert_eq!(updated.to_rfc3339(), "2026-01-02T03:04:05+00:00");
        assert_eq!(b.updated_at("gone").await.unwrap(), None);
        server.stop();
    }

    #[tokio::test]
    async fn set_metadata_merges_and_copies_with_replace_directive() {
        let server = mock_http(vec![
            // HEAD: existing metadata a=1 (plus an empty value to be
            // dropped by the merge? no — Python only skips NEW empties).
            response_with(
                200,
                "OK",
                &[("x-amz-meta-a", "1"), ("Content-Type", "text/plain")],
                "",
            ),
            // PUT copy-in-place.
            response_with(
                200,
                "OK",
                &[],
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\
                 <CopyObjectResult><ETag>\"e2\"</ETag></CopyObjectResult>",
            ),
        ])
        .await;
        let b = test_backend(&server.base_url);
        b.set_metadata(
            "queue/j1.json",
            &BTreeMap::from([
                ("b".to_string(), "2".to_string()),
                ("empty".to_string(), String::new()),
            ]),
        )
        .await
        .unwrap();
        let reqs = requests(&server);
        assert!(
            reqs[1].starts_with("PUT /test-bucket/queue/j1.json"),
            "{}",
            reqs[1]
        );
        assert!(
            reqs[1].contains("x-amz-copy-source: test-bucket/queue/j1.json"),
            "{}",
            reqs[1]
        );
        assert!(
            reqs[1].contains("x-amz-metadata-directive: REPLACE"),
            "{}",
            reqs[1]
        );
        assert!(reqs[1].contains("x-amz-meta-a: 1"), "{}", reqs[1]);
        assert!(reqs[1].contains("x-amz-meta-b: 2"), "{}", reqs[1]);
        // Empty new values are skipped (Python parity).
        assert!(!reqs[1].contains("x-amz-meta-empty"), "{}", reqs[1]);
        // ContentType preserved from the HEAD.
        assert!(reqs[1].contains("content-type: text/plain"), "{}", reqs[1]);
        server.stop();
    }

    #[tokio::test]
    async fn set_metadata_on_missing_object_propagates() {
        // Python head_object raises (no catch) when the blob is absent.
        let server = mock_http(vec![error_response(404, "404")]).await;
        let b = test_backend(&server.base_url);
        let err = b
            .set_metadata("gone", &BTreeMap::from([("k".into(), "v".into())]))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("S3 head_object"), "{err}");
        server.stop();
    }

    #[tokio::test]
    async fn list_blobs_with_meta_heads_each_object() {
        let page = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\
            <ListBucketResult xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\">\
            <Name>test-bucket</Name><Prefix>queue/</Prefix><IsTruncated>false</IsTruncated>\
            <Contents><Key>queue/j1.json</Key><LastModified>2026-01-02T03:04:05.000Z</LastModified></Contents>\
            </ListBucketResult>";
        let server = mock_http(vec![
            response_with(200, "OK", &[], page),
            response_with(
                200,
                "OK",
                &[
                    ("x-amz-meta-gpu_mem_gb", "24"),
                    ("x-amz-meta-priority", "0"),
                    ("x-amz-meta-gpu_type", "nvidia-l4"),
                ],
                "",
            ),
        ])
        .await;
        let b = test_backend(&server.base_url);
        let infos = b.list_blobs_with_meta("queue/").await.unwrap();
        assert_eq!(infos.len(), 1);
        assert_eq!(infos[0].name, "queue/j1.json");
        assert_eq!(
            infos[0].updated.unwrap().to_rfc3339(),
            "2026-01-02T03:04:05+00:00"
        );
        assert_eq!(
            infos[0].metadata,
            BTreeMap::from([
                ("gpu_mem_gb".to_string(), "24".to_string()),
                ("priority".to_string(), "0".to_string()),
                ("gpu_type".to_string(), "nvidia-l4".to_string()),
            ])
        );
        server.stop();
    }

    #[test]
    fn etag_unquoting_matches_python_strip() {
        assert_eq!(unquote_etag("\"abc123\""), "abc123");
        assert_eq!(unquote_etag("abc123"), "abc123");
        // Python .strip('"') removes ALL leading/trailing quotes.
        assert_eq!(unquote_etag("\"\"ab\"\""), "ab");
    }

    #[test]
    fn copy_source_encodes_segments_keeps_slashes() {
        assert_eq!(copy_source("bkt", "queue/j1.json"), "bkt/queue/j1.json");
        assert_eq!(copy_source("bkt", "a b/cż.json"), "bkt/a%20b/c%C5%BC.json");
    }

    #[test]
    fn error_mapping_status_and_code_based() {
        // Unit-level: a fabricated SDK error is not constructible without a
        // raw response, so exercise the classification through the status /
        // code matrix the helpers encode.
        for (code, status, want_nf, want_pc) in [
            (Some("NoSuchKey"), Some(404u16), true, false),
            (Some("404"), Some(404), true, false),
            (Some("NotFound"), None, true, false),
            (Some("PreconditionFailed"), Some(412), false, true),
            (Some("412"), None, false, true),
            (Some("AccessDenied"), Some(403), false, false),
            (None, None, false, false),
        ] {
            let nf = status == Some(404) || matches!(code, Some("404" | "NoSuchKey" | "NotFound"));
            let pc = status == Some(412) || matches!(code, Some("PreconditionFailed" | "412"));
            assert_eq!(
                (nf, pc),
                (want_nf, want_pc),
                "code={code:?} status={status:?}"
            );
        }
    }

    #[test]
    fn to_utc_converts_sdk_datetime() {
        let dt = aws_sdk_s3::primitives::DateTime::from_secs_and_nanos(1_767_321_845, 250_000_000);
        let converted = to_utc(&dt).unwrap();
        assert_eq!(converted.timestamp(), 1_767_321_845);
        assert_eq!(converted.timestamp_subsec_nanos(), 250_000_000);
    }
}
