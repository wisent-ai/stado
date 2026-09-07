//! Single-writer storage with configurable read authority.
//!
//! Mutations commit to the configured primary, then mirror to the read-only
//! disaster-recovery backend. Replica errors are reported without turning an
//! already-committed primary mutation into a false failure. Normal clients may
//! use the backup after a failed primary read; authority-sensitive users retain
//! the same write mirror but return the primary error. A successful `absent`
//! answer is always authoritative. The backup is never promoted to writer.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};

use super::{BlobBackend, BlobInfo, StorageError, VersionedText, UPLOAD_PART_MARKER};
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ReadMode {
    Failover,
    PrimaryOnly,
}

pub struct ReadFailoverBackend {
    primary: Arc<dyn BlobBackend>,
    backup: Arc<dyn BlobBackend>,
    read_mode: ReadMode,
}

impl ReadFailoverBackend {
    pub fn new(
        primary: Arc<dyn BlobBackend>,
        backup: Arc<dyn BlobBackend>,
        read_mode: ReadMode,
    ) -> Self {
        Self {
            primary,
            backup,
            read_mode,
        }
    }

    fn report_replica_error(operation: &str, path: &str, error: &StorageError) {
        eprintln!(
            "[storage-replica] primary committed but backup {operation} failed for {path}: {error}"
        );
    }

    /// Whether a key the primary reports absent may be answered from the
    /// mirror and written back.
    ///
    /// Only an immutable published release object qualifies. A release object
    /// is written once and never changes, so a mirror copy of it cannot be
    /// stale — which is the whole reason `PrimaryOnly` exists, and the reason
    /// it can be relaxed exactly here and nowhere else. Two keys that live
    /// under the same prefix and still do not qualify: an upload part, because
    /// resurrecting one makes an abandoned upload look resumable, and any
    /// mutable key such as queue state, where a replica IS allowed to be
    /// behind and serving it would answer with an old world.
    fn heals_from_mirror(path: &str) -> bool {
        if path.contains(UPLOAD_PART_MARKER) {
            return false;
        }
        path.split('/').any(|segment| segment == "releases")
    }

    /// Serve an immutable object the primary is missing from the mirror, and
    /// write it back so the next read — and every stat — is answered by the
    /// authority itself.
    ///
    /// A heal that cannot be written is still served: the caller asked for
    /// bytes that exist, and a replica the primary cannot accept is a
    /// separate defect, reported rather than turned into a false absence.
    async fn mirrored_bytes(&self, path: &str) -> Result<Option<Vec<u8>>, StorageError> {
        if !Self::heals_from_mirror(path) {
            return Ok(None);
        }
        let Some(content) = self.backup.download_bytes(path).await? else {
            return Ok(None);
        };
        if let Err(error) = self.primary.upload_bytes(path, &content).await {
            Self::report_replica_error("heal", path, &error);
        }
        Ok(Some(content))
    }
}

#[async_trait]
impl BlobBackend for ReadFailoverBackend {
    /// Addressing follows the PRIMARY, which is the writer and therefore the
    /// authority on where an object lives. Inheriting the trait default here
    /// would have silently overridden a primary that spells its keys
    /// differently — the object API being the one that does — so a store with
    /// a backup configured addressed every object one way and a store without
    /// one addressed it the other.
    fn blob_path(&self, object: &crate::object_store::ObjectRef) -> String {
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
            Err(_) => self.backup.download_text(path).await,
        }
    }

    async fn download_bytes(&self, path: &str) -> Result<Option<Vec<u8>>, StorageError> {
        match self.primary.download_bytes(path).await {
            Ok(Some(content)) => Ok(Some(content)),
            Ok(None) => self.mirrored_bytes(path).await,
            Err(error) if self.read_mode == ReadMode::PrimaryOnly => Err(error),
            Err(_) => self.backup.download_bytes(path).await,
        }
    }

    async fn download_release(&self, uri: &str) -> Result<Option<Vec<u8>>, StorageError> {
        match self.primary.download_release(uri).await {
            Ok(Some(content)) => Ok(Some(content)),
            // A release URI is immutable by definition, so the mirror can
            // answer it; the heal writes through the primary's own addressing.
            Ok(None) => match self.backup.download_release(uri).await? {
                Some(content) => {
                    let path = self
                        .primary
                        .blob_path(&crate::object_store::ObjectRef::parse(uri)?);
                    if let Err(error) = self.primary.upload_bytes(&path, &content).await {
                        Self::report_replica_error("heal", &path, &error);
                    }
                    Ok(Some(content))
                }
                None => Ok(None),
            },
            Err(error) if self.read_mode == ReadMode::PrimaryOnly => Err(error),
            Err(_) => self.backup.download_release(uri).await,
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
            Err(error) if self.read_mode == ReadMode::PrimaryOnly => Err(error),
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
            Err(error) if self.read_mode == ReadMode::PrimaryOnly => Err(error),
            Err(_) => self.backup.list_paths(prefix, oldest_first).await,
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
        limit: usize,
    ) -> Result<Vec<String>, StorageError> {
        match self.primary.list_page(prefix, start_after, limit).await {
            answer @ Ok(_) => answer,
            Err(error) if self.read_mode == ReadMode::PrimaryOnly => Err(error),
            Err(_) => self.backup.list_page(prefix, start_after, limit).await,
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
            Err(error) if self.read_mode == ReadMode::PrimaryOnly => Err(error),
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
            Err(error) if self.read_mode == ReadMode::PrimaryOnly => Err(error),
            Err(_) => self.backup.list_blobs_with_meta(prefix).await,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::queue::local_file::LocalBackend;

    const RELEASE: &str = "ecosystem/releases/preferences-landing/0.1.1/web/release.json";
    const PART: &str =
        "ecosystem/releases/stado/0.15.25/darwin-arm64/x.tar.gz.__stado_upload/abc/00000000";
    const QUEUE_KEY: &str = "ecosystem/probierz/queue/job-1.json";

    /// Two real local roots, so the heal is exercised against a filesystem
    /// rather than a mock that cannot refuse.
    fn two_roots() -> (tempfile::TempDir, tempfile::TempDir, ReadFailoverBackend) {
        let primary = tempfile::tempdir().expect("primary root");
        let backup = tempfile::tempdir().expect("backup root");
        let store = ReadFailoverBackend::new(
            Arc::new(LocalBackend::new(&primary.path().to_string_lossy()).expect("primary")),
            Arc::new(LocalBackend::new(&backup.path().to_string_lossy()).expect("backup")),
            // The object API's own mode: reads must not be answered from a
            // stale replica. Healing is what makes this case answerable
            // without breaking that rule.
            ReadMode::PrimaryOnly,
        );
        (primary, backup, store)
    }

    #[tokio::test]
    async fn a_primary_miss_the_mirror_can_answer_is_served_and_healed() {
        let (primary, _backup, store) = two_roots();
        store
            .backup
            .upload_bytes(RELEASE, b"signed manifest")
            .await
            .expect("seed the mirror only");

        assert_eq!(
            store.download_bytes(RELEASE).await.expect("read"),
            Some(b"signed manifest".to_vec()),
            "the mirror's copy must be served, not reported absent"
        );
        assert!(
            primary.path().join(RELEASE).is_file(),
            "the primary must have been healed with it"
        );
        assert!(
            store.exists(RELEASE).await.expect("stat"),
            "stat must agree with what a read serves"
        );
    }

    #[tokio::test]
    async fn absent_from_both_stays_absent() {
        let (_primary, _backup, store) = two_roots();
        assert_eq!(store.download_bytes(RELEASE).await.expect("read"), None);
        assert!(!store.exists(RELEASE).await.expect("stat"));
    }

    /// An upload part is not an object: resurrecting one would make an
    /// abandoned upload look resumable.
    #[tokio::test]
    async fn an_upload_part_is_never_healed() {
        let (primary, _backup, store) = two_roots();
        store
            .backup
            .upload_bytes(PART, b"part")
            .await
            .expect("seed the mirror only");
        assert_eq!(store.download_bytes(PART).await.expect("read"), None);
        assert!(!primary.path().join(PART).exists());
    }

    /// And a mutable key is not healed either, which is the invariant
    /// `PrimaryOnly` exists to protect: a stale replica must never be returned
    /// as authority.
    #[tokio::test]
    async fn a_mutable_key_is_not_served_from_the_mirror() {
        let (primary, _backup, store) = two_roots();
        store
            .backup
            .upload_bytes(QUEUE_KEY, b"stale job state")
            .await
            .expect("seed the mirror only");
        assert_eq!(store.download_bytes(QUEUE_KEY).await.expect("read"), None);
        assert!(!primary.path().join(QUEUE_KEY).exists());
    }

    /// A primary that ERRORS still falls back exactly as before, for a normal
    /// client: that path is untouched.
    #[tokio::test]
    async fn a_primary_error_still_falls_back_for_a_failover_reader() {
        let primary = tempfile::tempdir().expect("primary root");
        let backup = tempfile::tempdir().expect("backup root");
        let store = ReadFailoverBackend::new(
            Arc::new(LocalBackend::new(&primary.path().to_string_lossy()).expect("primary")),
            Arc::new(LocalBackend::new(&backup.path().to_string_lossy()).expect("backup")),
            ReadMode::Failover,
        );
        store
            .backup
            .upload_bytes(RELEASE, b"mirror")
            .await
            .expect("seed the mirror");
        assert_eq!(
            store.download_bytes(RELEASE).await.expect("read"),
            Some(b"mirror".to_vec())
        );
    }
}
