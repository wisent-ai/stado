//! The paginated ListObjectsV2 walks: the windowed page the queue index is
//! read through, the full drain every other listing is built on, and the
//! per-key HEAD that turns a drain into blob metadata.
//!
//! `objects/mod.rs` carries the trait entries; the walks are here because
//! they share one continuation-token loop, and differ only in what bounds it
//! — a caller's window, or the end of the prefix. A listed object without a
//! key is refused, never read as an empty name.

use std::collections::BTreeMap;
use std::num::NonZeroUsize;

use aws_sdk_s3::types::Object;

use crate::queue::{BlobInfo, StorageError};

use super::super::{
    client::to_utc,
    refusals::{is_not_found, sdk_err},
    S3Backend,
};

/// The key of one listed object.
fn key(object: &Object) -> Result<&str, StorageError> {
    object.key().ok_or_else(|| {
        StorageError::Other(format!(
            "S3 ListObjectsV2 answered an object without a key: {object:?}"
        ))
    })
}

impl S3Backend {
    /// One window of keys under `prefix`: server-side ordered, resumed past
    /// `start_after`, and bounded by `limit` when one is given.
    pub(super) async fn list_objects_page(
        &self,
        prefix: &str,
        start_after: &str,
        limit: Option<NonZeroUsize>,
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
            if let Some(limit) = limit {
                // Only what is still missing. The service caps a page below
                // what an i32 holds, so a window wider than an i32 asks for
                // the most an i32 states and pages on for the rest.
                let remaining = (limit.get() - out.len()).min(i32::MAX as usize);
                let asked = i32::try_from(remaining).map_err(|error| {
                    StorageError::Other(format!(
                        "S3 page size {remaining} does not fit the request's key count: {error}"
                    ))
                })?;
                request = request.max_keys(asked);
            }
            if let Some(token) = &token {
                request = request.continuation_token(token);
            }
            let page = crate::wait::sdk(request.send())
                .await
                .map_err(|err| sdk_err("list_objects_v2", err))?;
            for object in page.contents() {
                out.push(key(object)?.to_string());
                if limit.is_some_and(|limit| out.len() >= limit.get()) {
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
    /// `prefix`, in listing order.
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
            let page = crate::wait::sdk(request.send())
                .await
                .map_err(|err| sdk_err("list_objects_v2", err))?;
            for object in page.contents() {
                out.push((key(object)?.to_string(), object.last_modified().cloned()));
            }
            match page.next_continuation_token() {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }
        Ok(out)
    }

    /// Every object under `prefix` with the size and timestamp its
    /// ListObjectsV2 entry states, and no metadata: one request per page,
    /// not one HEAD per object, for a caller that reads no metadata.
    pub(super) async fn list_objects_sized(
        &self,
        prefix: &str,
    ) -> Result<Vec<BlobInfo>, StorageError> {
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
            let page = crate::wait::sdk(request.send())
                .await
                .map_err(|err| sdk_err("list_objects_v2", err))?;
            for object in page.contents() {
                let name = key(object)?.to_string();
                let size = match object.size() {
                    None => None,
                    Some(length) => Some(u64::try_from(length).map_err(|error| {
                        StorageError::Other(format!(
                            "S3 ListObjectsV2 listed {name} with a size {length} that is no byte count: {error}"
                        ))
                    })?),
                };
                out.push(BlobInfo {
                    metadata: BTreeMap::new(),
                    updated: object.last_modified().and_then(to_utc),
                    size,
                    name,
                });
            }
            match page.next_continuation_token() {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }
        Ok(out)
    }

    /// One object's name, last-modified time, size and user metadata, read
    /// with one HEAD, or `None` when no object has that key.
    pub(super) async fn head_blob(&self, key: &str) -> Result<Option<BlobInfo>, StorageError> {
        let head = match crate::wait::sdk(
            self.inner
                .client
                .head_object()
                .bucket(&self.inner.bucket)
                .key(key)
                .send(),
        )
        .await
        {
            Ok(head) => head,
            Err(err) if is_not_found(&err) => return Ok(None),
            Err(err) => return Err(sdk_err("head_object", err)),
        };
        // An object written with no user metadata answers with none.
        let metadata: BTreeMap<String, String> = match head.metadata() {
            Some(fields) => fields
                .iter()
                .map(|(name, value)| (name.clone(), value.clone()))
                .collect(),
            None => BTreeMap::new(),
        };
        let size = match head.content_length() {
            None => None,
            Some(length) => Some(u64::try_from(length).map_err(|error| {
                StorageError::Other(format!(
                    "S3 HEAD {key} answered a content length {length} that is no byte count: {error}"
                ))
            })?),
        };
        Ok(Some(BlobInfo {
            metadata,
            updated: head.last_modified().and_then(to_utc),
            size,
            name: key.to_string(),
        }))
    }

    /// Every object under `prefix` with the metadata, size and timestamp
    /// recovery reads off it: one HEAD per listed key. A key the listing
    /// named and the HEAD no longer finds was deleted between the two, which
    /// is said, never read as an object with no metadata.
    pub(super) async fn list_blobs_meta(
        &self,
        prefix: &str,
    ) -> Result<Vec<BlobInfo>, StorageError> {
        let mut out = Vec::new();
        for (key, _) in self.list_objects(prefix).await? {
            match self.head_blob(&key).await? {
                Some(blob) => out.push(blob),
                None => {
                    return Err(StorageError::Other(format!(
                        "S3 listed {key} under {prefix}, and its HEAD found no such key: it was \
                         deleted while the prefix was read"
                    )))
                }
            }
        }
        Ok(out)
    }
}
