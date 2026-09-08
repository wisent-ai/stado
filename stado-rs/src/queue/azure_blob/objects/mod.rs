//! The [`BlobBackend`] surface: every queue storage operation the rest of
//! the crate reaches this backend through.
//!
//! The bodies that are a seam of their own live beside this file — the body
//! GET and the properties HEAD (`reads`), Put Blob and the conditional
//! header that turns it into a create (`writes`), the paginated List Blobs
//! walk (`listing`) and the response parse it feeds on (`xml`).

use std::collections::BTreeMap;
use std::path::Path;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use reqwest::{Method, StatusCode};

use crate::queue::{BlobBackend, BlobInfo, StorageError, VersionedText};

use super::{client::header_str, AzureBlobBackend, LIST_MAX_RESULTS};

mod listing;
mod reads;
mod writes;
mod xml;

use reads::GetOutcome;

#[async_trait]
impl BlobBackend for AzureBlobBackend {
    async fn upload_text(&self, path: &str, content: &str) -> Result<(), StorageError> {
        self.upload_bytes(path, content.as_bytes()).await
    }

    async fn upload_bytes(&self, path: &str, content: &[u8]) -> Result<(), StorageError> {
        let response = self.put_blob(path, content.to_vec(), None).await?;
        Self::ensure_success(response, &format!("PUT {path}")).await?;
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
        match self.get_bytes(path, None).await {
            GetOutcome::Ok(bytes) => Ok(Some(bytes)),
            GetOutcome::NotFound => Ok(None),
            GetOutcome::PreconditionFailed => unreachable!("unpinned GET cannot 412"),
            GetOutcome::Error(err) => Err(err),
        }
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
        // Python:
        // 3 attempts of (properties -> pinned download); a 412
        // between HEAD and GET retries, the final 412 re-raises.
        for attempt in 0..3 {
            let Some(props) = self.head(path).await? else {
                return Ok(None);
            };
            let etag = props.etag.unwrap_or_default();
            match self.get_bytes(path, Some(&etag)).await {
                GetOutcome::Ok(bytes) => {
                    let content = String::from_utf8(bytes).map_err(|err| {
                        StorageError::Other(format!("invalid UTF-8 in {path}: {err}"))
                    })?;
                    // Python returns str(props.etag) — the HEAD ETag,
                    // verbatim (quoted).
                    return Ok(Some(VersionedText {
                        content,
                        version: etag,
                    }));
                }
                GetOutcome::NotFound => return Ok(None),
                GetOutcome::PreconditionFailed if attempt < 2 => continue,
                GetOutcome::PreconditionFailed => {
                    // Python re-raises ResourceModifiedError here (NOT a
                    // StorageConflict).
                    return Err(StorageError::Other(format!(
                        "azure blob {path} changed during versioned read (3 attempts)"
                    )));
                }
                GetOutcome::Error(err) => return Err(err),
            }
        }
        Err(StorageError::Other(
            "unreachable versioned Azure Blob read".into(),
        ))
    }

    async fn compare_and_swap_text(
        &self,
        path: &str,
        expected_version: &str,
        content: &str,
    ) -> Result<String, StorageError> {
        let response = self
            .put_blob(
                path,
                content.as_bytes().to_vec(),
                Some(("If-Match", expected_version)),
            )
            .await?;
        if response.status() == StatusCode::PRECONDITION_FAILED {
            return Err(StorageError::StorageConflict(format!(
                "{path} changed concurrently"
            )));
        }
        let response = Self::ensure_success(response, &format!("conditional PUT {path}")).await?;
        // Python: response.get("etag") or RuntimeError.
        header_str(&response, "etag").ok_or_else(|| {
            StorageError::Other("Azure conditional write did not return its ETag".into())
        })
    }

    async fn delete(&self, path: &str) -> Result<(), StorageError> {
        let response = self
            .send(Method::DELETE, &self.blob_url(path), &[], None)
            .await?;
        if response.status() == StatusCode::NOT_FOUND {
            return Ok(());
        }
        Self::ensure_success(response, &format!("DELETE {path}")).await?;
        Ok(())
    }

    async fn exists(&self, path: &str) -> Result<bool, StorageError> {
        let response = self
            .send(Method::HEAD, &self.blob_url(path), &[], None)
            .await?;
        if response.status() == StatusCode::NOT_FOUND {
            return Ok(false);
        }
        Self::ensure_success(response, &format!("HEAD {path}")).await?;
        Ok(true)
    }

    async fn list_paths(
        &self,
        prefix: &str,
        oldest_first: usize,
    ) -> Result<Vec<String>, StorageError> {
        let mut entries = self.list_entries(prefix, false).await?;
        if oldest_first > 0 {
            // Python: sort by creation_time ascending (None sorts as
            // datetime.min), take the N oldest.
            entries.sort_by_key(|entry| entry.creation_time);
            entries.truncate(oldest_first);
        }
        Ok(entries.into_iter().map(|entry| entry.name).collect())
    }

    /// List Blobs returns blobs in lexicographic name order and accepts
    /// `maxresults` plus an opaque continuation `marker`; it has no
    /// `start-after`/`startOffset`, and the marker is not a blob name, so
    /// the exclusive cursor cannot be handed to the service. The window is
    /// therefore filled client-side, but the walk stops the moment it is
    /// full instead of doing what the generic default does — pull the whole
    /// prefix listing and cut it on every call, which for the 14k-blob
    /// queue prefix is three round trips and 14k names to answer a
    /// head-of-index poll that wants a handful. Name order is what makes
    /// the skip bounded: past `start_after` every later name qualifies, so
    /// the discard is a prefix of the walk rather than a filter over all of
    /// it. Resuming from a deep cursor still scans forward to reach it,
    /// since Azure cannot express the offset server-side, but these are
    /// name-only pages (no `include=metadata`, no bodies) — the cost that
    /// actually hurt was fetching content, not enumerating names.
    async fn list_page(
        &self,
        prefix: &str,
        start_after: &str,
        limit: usize,
    ) -> Result<Vec<String>, StorageError> {
        // A page smaller than the window would force extra round trips, and
        // a page sized to the window would do the same while skipping to a
        // cursor, whose names have to cross the wire either way. So ask for
        // exactly the window only when there is nothing to skip.
        let page = if limit == 0 || !start_after.is_empty() {
            LIST_MAX_RESULTS
        } else {
            limit.min(LIST_MAX_RESULTS)
        };
        // Uncapped listings grow as they go; a window pre-sizes to itself.
        let mut names: Vec<String> = Vec::with_capacity(limit.min(page));
        self.walk_list_pages(prefix, false, Some(page), |entries| {
            for entry in entries {
                if entry.name.as_str() <= start_after {
                    continue;
                }
                names.push(entry.name);
                if limit > 0 && names.len() >= limit {
                    return false;
                }
            }
            true
        })
        .await?;
        Ok(names)
    }

    async fn updated_at(&self, path: &str) -> Result<Option<DateTime<Utc>>, StorageError> {
        Ok(self.head(path).await?.and_then(|props| props.last_modified))
    }

    async fn set_metadata(
        &self,
        path: &str,
        kv: &BTreeMap<String, String>,
    ) -> Result<(), StorageError> {
        let Some(props) = self.head(path).await? else {
            return Err(StorageError::NotFound(path.to_string()));
        };
        let mut merged: BTreeMap<String, String> = props.metadata;
        merged.extend(
            kv.iter()
                .filter(|(_, v)| !v.is_empty())
                .map(|(k, v)| (k.clone(), v.clone())),
        );
        let headers: Vec<(String, String)> = merged
            .iter()
            .map(|(k, v)| (format!("x-ms-meta-{k}"), v.clone()))
            .collect();
        let header_refs: Vec<(&str, &str)> = headers
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        let url = format!("{}?comp=metadata", self.blob_url(path));
        let response = self.send(Method::PUT, &url, &header_refs, None).await?;
        Self::ensure_success(response, &format!("set_metadata {path}")).await?;
        Ok(())
    }

    async fn list_blobs_with_meta(&self, prefix: &str) -> Result<Vec<BlobInfo>, StorageError> {
        Ok(self
            .list_entries(prefix, true)
            .await?
            .into_iter()
            .map(|entry| BlobInfo {
                name: entry.name,
                updated: entry.last_modified,
                size: entry.size,
                metadata: entry.metadata,
            })
            .collect())
    }
}
