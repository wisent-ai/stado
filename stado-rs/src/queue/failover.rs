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

use super::{BlobBackend, BlobInfo, StorageError, VersionedText};
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

    /// A part of an unfinished multipart upload, which is not an object.
    const UPLOAD_PART_MARKER: &'static str = "__stado_upload";

    /// Whether this path names an object the primary may be healed with from
    /// the mirror.
    ///
    /// Deliberately narrow. Serving a mirror copy in place of an absent
    /// primary is what [`ReadMode::PrimaryOnly`] exists to refuse: for a
    /// mutable key a stale replica would be returned as authority. But a
    /// release object is immutable by contract — the object API accepts them
    /// create-only and refuses to delete them — so a mirror copy of one cannot
    /// be a stale version of anything. For those, and only those, a primary
    /// that has lost the object can be repaired from the copy that survived,
    /// after which the read is answered from the PRIMARY's own bytes and the
    /// authority rule is intact rather than bypassed.
    ///
    /// Upload parts are excluded because they are not objects: the composed
    /// object is, and a part resurrected on its own would make an abandoned
    /// upload look resumable.
    fn healable(path: &str) -> bool {
        let key = path
            .strip_prefix(crate::object_store::ROOT_PREFIX)
            .unwrap_or(path);
        key.starts_with("releases/") && !path.contains(Self::UPLOAD_PART_MARKER)
    }

    /// Repair the primary from the mirror, create-only, and say so once.
    ///
    /// On 2026-09-05 `preferences-landing` 0.1.1 published five objects, was
    /// read back complete at 13:18, and answered 404 for four of them from
    /// 13:23 — the minute its object API restarted. The four were still on
    /// disk in the mirror the whole time; nothing had deleted them from the
    /// world, and no DELETE ever reached the API. A clean primary `absent` was
    /// returned as fact, so a recoverable inconsistency read as a destroyed
    /// release, and the 104 MB archive that the mirror did NOT hold was lost
    /// before anybody knew to look.
    ///
    /// So the store heals on read instead of hiding: the copy that exists is
    /// written back where the reader looked for it.
    async fn heal_primary(&self, path: &str, content: &[u8]) {
        if !Self::healable(path) {
            return;
        }
        let staged = match tempfile::NamedTempFile::new() {
            Ok(staged) => staged,
            Err(error) => {
                eprintln!(
                    "[storage-replica] warn primary_absent_mirror_present {path}: cannot stage the \
                     mirror copy to heal the primary: {error}"
                );
                return;
            }
        };
        if let Err(error) = std::fs::write(staged.path(), content) {
            eprintln!(
                "[storage-replica] warn primary_absent_mirror_present {path}: cannot write the \
                 staged mirror copy: {error}"
            );
            return;
        }
        match self
            .primary
            .upload_file_if_absent(path, staged.path())
            .await
        {
            Ok(created) => eprintln!(
                "[storage-replica] warn primary_absent_mirror_present {path}: the primary answered \
                 absent and the mirror holds this immutable release object; healed the primary \
                 (created={created}) and served it"
            ),
            Err(error) => eprintln!(
                "[storage-replica] warn primary_absent_mirror_present {path}: the mirror holds it \
                 but the primary refused the repair: {error}"
            ),
        }
    }

    /// The mirror's bytes for a path the primary says it does not have, having
    /// healed the primary with them.
    async fn mirror_bytes_for_absent_primary(&self, path: &str) -> Option<Vec<u8>> {
        if !Self::healable(path) {
            return None;
        }
        let content = self.backup.download_bytes(path).await.ok().flatten()?;
        self.heal_primary(path, &content).await;
        Some(content)
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
            Ok(None) => match self.mirror_bytes_for_absent_primary(path).await {
                Some(content) => Ok(Some(String::from_utf8_lossy(&content).into_owned())),
                None => Ok(None),
            },
            answer @ Ok(_) => answer,
            Err(error) if self.read_mode == ReadMode::PrimaryOnly => Err(error),
            Err(_) => self.backup.download_text(path).await,
        }
    }

    async fn download_bytes(&self, path: &str) -> Result<Option<Vec<u8>>, StorageError> {
        match self.primary.download_bytes(path).await {
            Ok(None) => Ok(self.mirror_bytes_for_absent_primary(path).await),
            answer @ Ok(_) => answer,
            Err(error) if self.read_mode == ReadMode::PrimaryOnly => Err(error),
            Err(_) => self.backup.download_bytes(path).await,
        }
    }

    async fn download_release(&self, uri: &str) -> Result<Option<Vec<u8>>, StorageError> {
        match self.primary.download_release(uri).await {
            Ok(None) => {
                // The release route addresses by URI; the heal writes by path,
                // so the primary's own addressing decides where the repair
                // lands, exactly as `blob_path` does for every other write.
                let Ok(object) = crate::object_store::ObjectRef::parse(uri) else {
                    return Ok(None);
                };
                let path = self.primary.blob_path(&object);
                Ok(self.mirror_bytes_for_absent_primary(&path).await)
            }
            answer @ Ok(_) => answer,
            Err(error) if self.read_mode == ReadMode::PrimaryOnly => Err(error),
            Err(_) => self.backup.download_release(uri).await,
        }
    }

    async fn download_to_filename(&self, path: &str, dest: &Path) -> Result<bool, StorageError> {
        match self.primary.download_to_filename(path, dest).await {
            Ok(false) => match self.mirror_bytes_for_absent_primary(path).await {
                Some(content) => {
                    std::fs::write(dest, &content).map_err(StorageError::from)?;
                    Ok(true)
                }
                None => Ok(false),
            },
            answer @ Ok(_) => answer,
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

    async fn exists(&self, path: &str) -> Result<bool, StorageError> {
        let exact = |blobs: Vec<BlobInfo>| blobs.into_iter().any(|blob| blob.name == path);
        match self.primary.list_blobs_with_meta(path).await {
            // `stat` must not answer `absent` for something a read would
            // serve: that disagreement is what made a recoverable
            // inconsistency look like a destroyed release. Healing here means
            // the next reader finds it in the primary.
            Ok(blobs) => {
                if exact(blobs) {
                    return Ok(true);
                }
                Ok(self.mirror_bytes_for_absent_primary(path).await.is_some())
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
