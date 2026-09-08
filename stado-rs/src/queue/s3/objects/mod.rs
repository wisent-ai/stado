//! The [`BlobBackend`] surface: every queue storage operation the rest of
//! the crate reaches this backend through.
//!
//! The bodies that are a seam of their own live beside this file — the
//! GetObject that answers with a body and the versioned read above it
//! (`reads`), the if-absent create, the compare-and-swap and the metadata
//! copy-in-place (`writes`), and the paginated ListObjectsV2 walks
//! (`listing`).

use std::collections::BTreeMap;
use std::path::Path;

use async_trait::async_trait;
use aws_sdk_s3::primitives::ByteStream;
use chrono::{DateTime, Utc};

use crate::queue::{BlobBackend, BlobInfo, StorageError, VersionedText};

use super::{
    client::to_utc,
    refusals::{is_not_found, sdk_err},
    S3Backend,
};

mod listing;
mod reads;
mod writes;

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
        self.get_object_bytes(path).await
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
        self.download_versioned_text(path).await
    }

    async fn compare_and_swap_text(
        &self,
        path: &str,
        expected_version: &str,
        content: &str,
    ) -> Result<String, StorageError> {
        self.compare_and_swap_object(path, expected_version, content)
            .await
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

    /// ListObjectsV2 states this contract natively: keys come back in
    /// ascending UTF-8 order, `start-after` is already an exclusive cursor,
    /// and `max-keys` bounds the response on the server. The generic default
    /// instead drains every continuation token of the prefix and sorts the
    /// result locally, so reading the head of the 14k-blob `queue/` index cost
    /// 15 list round-trips and a 14k-element sort per poll.
    async fn list_page(
        &self,
        prefix: &str,
        start_after: &str,
        limit: usize,
    ) -> Result<Vec<String>, StorageError> {
        self.list_objects_page(prefix, start_after, limit).await
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
        self.copy_object_with_metadata(path, kv).await
    }

    async fn list_blobs_with_meta(&self, prefix: &str) -> Result<Vec<BlobInfo>, StorageError> {
        self.list_blobs_meta(prefix).await
    }
}
