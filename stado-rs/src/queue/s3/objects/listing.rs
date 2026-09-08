//! The paginated ListObjectsV2 walks: the windowed page the queue index is
//! read through, the full drain every other listing is built on, and the
//! per-key HEAD that turns a drain into blob metadata.
//!
//! `objects/mod.rs` carries the trait entries; the walks are here because
//! they share one continuation-token loop, and differ only in what bounds it
//! — a caller's window, or the end of the prefix.

use crate::queue::{BlobInfo, StorageError};

use super::super::{client::to_utc, refusals::sdk_err, S3Backend};

impl S3Backend {
    /// One window of keys under `prefix`: server-side ordered, resumed past
    /// `start_after`, and bounded by `limit` (0 = unbounded).
    pub(super) async fn list_objects_page(
        &self,
        prefix: &str,
        start_after: &str,
        limit: usize,
    ) -> Result<Vec<String>, StorageError> {
        let mut out: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut request = self
                .inner
                .client
                .list_objects_v2()
                .bucket(&self.inner.bucket)
                .prefix(prefix);
            if !start_after.is_empty() {
                // Ignored by S3 once a continuation token is present, which is
                // correct: the token already resumes past everything returned.
                request = request.start_after(start_after);
            }
            if limit > 0 {
                // Only what is still missing. S3 caps a page at 1000 anyway,
                // so the saturating cast never loses a needed key.
                let remaining = limit - out.len();
                request = request.max_keys(i32::try_from(remaining).unwrap_or(i32::MAX));
            }
            if let Some(token) = &token {
                request = request.continuation_token(token);
            }
            let page = request
                .send()
                .await
                .map_err(|err| sdk_err("list_objects_v2", err))?;
            for object in page.contents() {
                out.push(object.key().unwrap_or_default().to_string());
                if limit > 0 && out.len() >= limit {
                    return Ok(out);
                }
            }
            match page.next_continuation_token() {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }
        Ok(out)
    }

    /// Paginated ListObjectsV2: (key, last_modified) for every object under
    /// `prefix`, in listing order (Python `_objects`).
    pub(super) async fn list_objects(
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

    /// Every object under `prefix` with the metadata, size and timestamp
    /// recovery reads off it.
    pub(super) async fn list_blobs_meta(
        &self,
        prefix: &str,
    ) -> Result<Vec<BlobInfo>, StorageError> {
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
