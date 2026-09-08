//! The [`BlobBackend`] surface: every queue storage operation the rest of
//! the crate reaches this backend through.
//!
//! The bodies that are a seam of their own live beside this file — the
//! resource and media GETs and the generation-pinned read above them
//! (`reads`), the media upload, the compare-and-swap it becomes under a
//! precondition and the metadata patch (`writes`), and the paginated
//! `objects.list` walks (`listing`).

use std::collections::BTreeMap;
use std::path::Path;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use reqwest::{Method, StatusCode};

use crate::queue::{BlobBackend, BlobInfo, StorageError, VersionedText};

use super::{client::parse_timestamp, refusals::ensure_success, uri::object_url, GcsBackend};

mod listing;
mod reads;
mod writes;

#[async_trait]
impl BlobBackend for GcsBackend {
    async fn upload_text(&self, path: &str, content: &str) -> Result<(), StorageError> {
        let response = self.upload(path, content.as_bytes().to_vec(), None).await?;
        ensure_success(response).await?;
        Ok(())
    }

    async fn upload_bytes(&self, path: &str, content: &[u8]) -> Result<(), StorageError> {
        let response = self.upload(path, content.to_vec(), None).await?;
        ensure_success(response).await?;
        Ok(())
    }

    async fn download_text(&self, path: &str) -> Result<Option<String>, StorageError> {
        let Some(bytes) = self.get_media(path, None).await? else {
            return Ok(None);
        };
        String::from_utf8(bytes)
            .map(Some)
            .map_err(|err| StorageError::Other(format!("invalid UTF-8 in {path}: {err}")))
    }

    async fn download_bytes(&self, path: &str) -> Result<Option<Vec<u8>>, StorageError> {
        self.get_media(path, None).await
    }

    async fn download_to_filename(&self, path: &str, dest: &Path) -> Result<bool, StorageError> {
        let Some(bytes) = self.get_media(path, None).await? else {
            return Ok(false);
        };
        std::fs::write(dest, bytes)?;
        Ok(true)
    }

    async fn upload_text_if_absent(&self, path: &str, content: &str) -> Result<bool, StorageError> {
        let response = self
            .upload(path, content.as_bytes().to_vec(), Some("0"))
            .await?;
        if response.status() == StatusCode::PRECONDITION_FAILED {
            return Ok(false);
        }
        ensure_success(response).await?;
        Ok(true)
    }

    async fn upload_file_if_absent(
        &self,
        path: &str,
        local_file: &Path,
    ) -> Result<bool, StorageError> {
        let bytes = std::fs::read(local_file)?;
        let response = self.upload(path, bytes, Some("0")).await?;
        if response.status() == StatusCode::PRECONDITION_FAILED {
            return Ok(false);
        }
        ensure_success(response).await?;
        Ok(true)
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
        self.swap_text_if_generation(path, expected_version, content)
            .await
    }

    async fn delete(&self, path: &str) -> Result<(), StorageError> {
        let url = object_url(&self.inner.bucket, path);
        let response = self.send(Method::DELETE, &url, None).await?;
        if response.status() == StatusCode::NOT_FOUND {
            return Ok(());
        }
        ensure_success(response).await?;
        Ok(())
    }

    async fn exists(&self, path: &str) -> Result<bool, StorageError> {
        Ok(self.get_object(path).await?.is_some())
    }

    async fn list_paths(
        &self,
        prefix: &str,
        oldest_first: usize,
    ) -> Result<Vec<String>, StorageError> {
        self.list_sorted_paths(prefix, oldest_first).await
    }

    async fn list_page(
        &self,
        prefix: &str,
        start_after: &str,
        limit: usize,
    ) -> Result<Vec<String>, StorageError> {
        self.list_name_page(prefix, start_after, limit).await
    }

    async fn updated_at(&self, path: &str) -> Result<Option<DateTime<Utc>>, StorageError> {
        let Some(object) = self.get_object(path).await? else {
            return Ok(None);
        };
        Ok(object.get("updated").and_then(parse_timestamp))
    }

    async fn set_metadata(
        &self,
        path: &str,
        kv: &BTreeMap<String, String>,
    ) -> Result<(), StorageError> {
        self.patch_metadata(path, kv).await
    }

    async fn list_blobs_with_meta(&self, prefix: &str) -> Result<Vec<BlobInfo>, StorageError> {
        self.list_blobs(prefix).await
    }
}
