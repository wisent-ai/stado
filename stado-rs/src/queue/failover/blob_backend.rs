//! The [`BlobBackend`] surface of the read-authority store.

use std::collections::BTreeMap;
use std::num::NonZeroUsize;
use std::path::Path;

use async_trait::async_trait;
use chrono::{DateTime, Utc};

use crate::queue::{BlobBackend, BlobInfo, Listing, StorageError, VersionedText};

use super::{ReadFailoverBackend, ReadMode};

#[async_trait]
impl BlobBackend for ReadFailoverBackend {
    /// Addressing follows the PRIMARY, which is the writer and therefore the
    /// authority on where an object lives. Inheriting the trait default here
    /// would have silently overridden a primary that spells its keys
    /// differently — the object API being the one that does — so a store with
    /// a backup configured addressed every object one way and a store without
    /// one addressed it the other.
    fn blob_path(&self, object: &crate::remote::object_store::ObjectRef) -> String {
        self.primary.blob_path(object)
    }

    fn blob_prefix(&self, namespace: &str, prefix: &str) -> Result<String, StorageError> {
        self.primary.blob_prefix(namespace, prefix)
    }

    async fn upload_text(&self, path: &str, content: &str) -> Result<(), StorageError> {
        self.primary.upload_text(path, content).await?;
        if let Err(error) = self.backup.upload_text(path, content).await {
            Self::report_replica_error("write", path, &error);
        }
        Ok(())
    }

    async fn upload_bytes(&self, path: &str, content: &[u8]) -> Result<(), StorageError> {
        self.primary.upload_bytes(path, content).await?;
        if let Err(error) = self.backup.upload_bytes(path, content).await {
            Self::report_replica_error("write", path, &error);
        }
        Ok(())
    }

    async fn download_text(&self, path: &str) -> Result<Option<String>, StorageError> {
        match self.primary.download_text(path).await {
            Ok(Some(text)) => Ok(Some(text)),
            Ok(None) => match self.mirrored_bytes(path).await? {
                Some(content) => String::from_utf8(content).map(Some).map_err(|error| {
                    StorageError::Other(format!("{path}: mirrored copy is not text: {error}"))
                }),
                None => Ok(None),
            },
            Err(error) if self.read_mode == ReadMode::PrimaryOnly => Err(error),
            Err(e) => Self::failed_over("download", path, e, self.backup.download_text(path).await),
        }
    }

    async fn download_bytes(&self, path: &str) -> Result<Option<Vec<u8>>, StorageError> {
        match self.primary.download_bytes(path).await {
            Ok(Some(content)) => Ok(Some(content)),
            Ok(None) => self.mirrored_bytes(path).await,
            Err(error) if self.read_mode == ReadMode::PrimaryOnly => Err(error),
            Err(e) => {
                let backup = self.backup.download_bytes(path).await;
                Self::failed_over("download", path, e, backup)
            }
        }
    }

    async fn read_range(
        &self,
        path: &str,
        first: u64,
        last: u64,
    ) -> Result<Option<Vec<u8>>, StorageError> {
        match self.primary.read_range(path, first, last).await {
            Ok(Some(content)) => Ok(Some(content)),
            Ok(None) if Self::heals_from_mirror(path) => {
                self.backup.read_range(path, first, last).await
            }
            Ok(None) => Ok(None),
            Err(error) if self.read_mode == ReadMode::PrimaryOnly => Err(error),
            Err(e) => {
                let backup = self.backup.read_range(path, first, last).await;
                Self::failed_over("ranged read", path, e, backup)
            }
        }
    }

    async fn download_release(&self, uri: &str) -> Result<Option<Vec<u8>>, StorageError> {
        match self.primary.download_release(uri).await {
            Ok(Some(content)) => Ok(Some(content)),
            Ok(None) => self.mirrored_release(uri).await,
            Err(error) if self.read_mode == ReadMode::PrimaryOnly => Err(error),
            Err(e) => {
                let backup = self.backup.download_release(uri).await;
                Self::failed_over("release download", uri, e, backup)
            }
        }
    }

    async fn download_to_filename(&self, path: &str, dest: &Path) -> Result<bool, StorageError> {
        match self.primary.download_to_filename(path, dest).await {
            Ok(true) => Ok(true),
            Ok(false) => match self.mirrored_bytes(path).await? {
                Some(content) => {
                    std::fs::write(dest, &content)?;
                    Ok(true)
                }
                None => Ok(false),
            },
            Err(error) if self.read_mode == ReadMode::PrimaryOnly => Err(error),
            Err(e) => {
                let backup = self.backup.download_to_filename(path, dest).await;
                Self::failed_over("download", path, e, backup)
            }
        }
    }

    async fn upload_text_if_absent(&self, path: &str, content: &str) -> Result<bool, StorageError> {
        let created = self.primary.upload_text_if_absent(path, content).await?;
        if created {
            if let Err(error) = self.backup.upload_text(path, content).await {
                Self::report_replica_error("create", path, &error);
            }
        }
        Ok(created)
    }

    async fn upload_file_if_absent(
        &self,
        path: &str,
        local_file: &Path,
    ) -> Result<bool, StorageError> {
        let created = self.primary.upload_file_if_absent(path, local_file).await?;
        if created {
            match std::fs::read(local_file) {
                Ok(content) => {
                    if let Err(error) = self.backup.upload_bytes(path, &content).await {
                        Self::report_replica_error("create", path, &error);
                    }
                }
                Err(error) => eprintln!(
                    "[storage-replica] primary committed but backup create could not read {}: {error}",
                    local_file.display()
                ),
            }
        }
        Ok(created)
    }

    async fn download_text_versioned(
        &self,
        path: &str,
    ) -> Result<Option<VersionedText>, StorageError> {
        match self.primary.download_text_versioned(path).await {
            answer @ Ok(_) => answer,
            Err(error) if self.read_mode == ReadMode::PrimaryOnly => Err(error),
            Err(e) => {
                let backup = self.backup.download_text_versioned(path).await;
                Self::failed_over("versioned read", path, e, backup)
            }
        }
    }

    async fn compare_and_swap_text(
        &self,
        path: &str,
        expected_version: &str,
        content: &str,
    ) -> Result<String, StorageError> {
        let version = self
            .primary
            .compare_and_swap_text(path, expected_version, content)
            .await?;
        if let Err(error) = self.backup.upload_text(path, content).await {
            Self::report_replica_error("compare-and-swap", path, &error);
        }
        Ok(version)
    }

    async fn delete(&self, path: &str) -> Result<(), StorageError> {
        self.primary.delete(path).await?;
        if let Err(error) = self.backup.delete(path).await {
            Self::report_replica_error("delete", path, &error);
        }
        Ok(())
    }

    /// Stat answers what a read would serve, which for an immutable object
    /// the primary is missing means asking the mirror as well. A stat that
    /// reports absent for bytes the next read hands over is an instrument
    /// disagreeing with the thing it measures.
    async fn exists(&self, path: &str) -> Result<bool, StorageError> {
        let exact = |blobs: Vec<BlobInfo>| blobs.into_iter().any(|blob| blob.name == path);
        match self.primary.list_blobs_with_meta(path).await {
            Ok(blobs) => {
                if exact(blobs) {
                    Ok(true)
                } else if Self::heals_from_mirror(path) {
                    self.backup.exists(path).await
                } else {
                    Ok(false)
                }
            }
            Err(error) if self.read_mode == ReadMode::PrimaryOnly => Err(error),
            Err(e) => {
                let backup = self.backup.list_blobs_with_meta(path).await.map(exact);
                Self::failed_over("existence check", path, e, backup)
            }
        }
    }

    async fn list_paths(
        &self,
        prefix: &str,
        listing: Listing,
    ) -> Result<Vec<String>, StorageError> {
        match self.primary.list_paths(prefix, listing).await {
            answer @ Ok(_) => answer,
            Err(error) if self.read_mode == ReadMode::PrimaryOnly => Err(error),
            Err(e) => {
                let backup = self.backup.list_paths(prefix, listing).await;
                Self::failed_over("listing", prefix, e, backup)
            }
        }
    }

    /// Delegated rather than inherited, because inheriting the trait default
    /// would erase the delegation: the default reaches for `list_paths` on
    /// THIS backend, so a primary with a server-side paged listing would have
    /// its page request degraded into a whole-prefix fetch plus a local cut.
    /// Forwarding keeps native paging and applies the selected read-authority
    /// rule to the primary result.
    async fn list_page(
        &self,
        prefix: &str,
        start_after: &str,
        limit: Option<NonZeroUsize>,
    ) -> Result<Vec<String>, StorageError> {
        match self.primary.list_page(prefix, start_after, limit).await {
            answer @ Ok(_) => answer,
            Err(error) if self.read_mode == ReadMode::PrimaryOnly => Err(error),
            Err(e) => {
                let backup = self.backup.list_page(prefix, start_after, limit).await;
                Self::failed_over("page listing", prefix, e, backup)
            }
        }
    }

    async fn updated_at(&self, path: &str) -> Result<Option<DateTime<Utc>>, StorageError> {
        Ok(self.stat(path).await?.and_then(|blob| blob.updated))
    }

    async fn stat(&self, path: &str) -> Result<Option<BlobInfo>, StorageError> {
        match self.primary.stat(path).await {
            Ok(blob) => Ok(blob),
            Err(error) if self.read_mode == ReadMode::PrimaryOnly => Err(error),
            Err(e) => {
                let backup = self.backup.stat(path).await;
                Self::failed_over("stat", path, e, backup)
            }
        }
    }

    async fn set_metadata(
        &self,
        path: &str,
        kv: &BTreeMap<String, String>,
    ) -> Result<(), StorageError> {
        self.primary.set_metadata(path, kv).await?;
        if let Err(error) = self.backup.set_metadata(path, kv).await {
            Self::report_replica_error("metadata update", path, &error);
        }
        Ok(())
    }

    async fn list_blobs_with_meta(&self, prefix: &str) -> Result<Vec<BlobInfo>, StorageError> {
        match self.primary.list_blobs_with_meta(prefix).await {
            answer @ Ok(_) => answer,
            Err(error) if self.read_mode == ReadMode::PrimaryOnly => Err(error),
            Err(e) => {
                let backup = self.backup.list_blobs_with_meta(prefix).await;
                Self::failed_over("metadata listing", prefix, e, backup)
            }
        }
    }

    /// The primary is the writer, so its changes are the ones to wait for.
    fn watch_prefixes(
        &self,
        prefixes: &[&str],
    ) -> Result<Box<dyn crate::queue::ChangeWatch>, StorageError> {
        self.primary.watch_prefixes(prefixes)
    }
}
