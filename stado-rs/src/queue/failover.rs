//! Single-writer storage with automatic read failover.
//!
//! Mutations commit to the configured primary, then mirror to the read-only
//! disaster-recovery backend. Replica errors are reported without turning an
//! already-committed primary mutation into a false failure. A failed primary
//! read may use the backup; a successful `absent` answer remains authoritative.
//! The backup is never promoted to writer, preventing split brain.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};

use super::{BlobBackend, BlobInfo, StorageError, VersionedText};

pub struct ReadFailoverBackend {
    primary: Arc<dyn BlobBackend>,
    backup: Arc<dyn BlobBackend>,
}

impl ReadFailoverBackend {
    pub fn new(primary: Arc<dyn BlobBackend>, backup: Arc<dyn BlobBackend>) -> Self {
        Self { primary, backup }
    }

    fn report_replica_error(operation: &str, path: &str, error: &StorageError) {
        eprintln!(
            "[storage-replica] primary committed but backup {operation} failed for {path}: {error}"
        );
    }
}

#[async_trait]
impl BlobBackend for ReadFailoverBackend {
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
            answer @ Ok(_) => answer,
            Err(_) => self.backup.download_text(path).await,
        }
    }

    async fn download_bytes(&self, path: &str) -> Result<Option<Vec<u8>>, StorageError> {
        match self.primary.download_bytes(path).await {
            answer @ Ok(_) => answer,
            Err(_) => self.backup.download_bytes(path).await,
        }
    }

    async fn download_to_filename(&self, path: &str, dest: &Path) -> Result<bool, StorageError> {
        match self.primary.download_to_filename(path, dest).await {
            answer @ Ok(_) => answer,
            Err(_) => self.backup.download_to_filename(path, dest).await,
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
            Err(_) => self.backup.download_text_versioned(path).await,
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

    async fn exists(&self, path: &str) -> Result<bool, StorageError> {
        let exact = |blobs: Vec<BlobInfo>| blobs.into_iter().any(|blob| blob.name == path);
        match self.primary.list_blobs_with_meta(path).await {
            Ok(blobs) => Ok(exact(blobs)),
            Err(_) => self.backup.list_blobs_with_meta(path).await.map(exact),
        }
    }

    async fn list_paths(
        &self,
        prefix: &str,
        oldest_first: usize,
    ) -> Result<Vec<String>, StorageError> {
        match self.primary.list_paths(prefix, oldest_first).await {
            answer @ Ok(_) => answer,
            Err(_) => self.backup.list_paths(prefix, oldest_first).await,
        }
    }

    async fn updated_at(&self, path: &str) -> Result<Option<DateTime<Utc>>, StorageError> {
        let exact = |blobs: Vec<BlobInfo>| {
            blobs
                .into_iter()
                .find(|blob| blob.name == path)
                .and_then(|blob| blob.updated)
        };
        match self.primary.list_blobs_with_meta(path).await {
            Ok(blobs) => Ok(exact(blobs)),
            Err(_) => self.backup.list_blobs_with_meta(path).await.map(exact),
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
            Err(_) => self.backup.list_blobs_with_meta(prefix).await,
        }
    }
}
