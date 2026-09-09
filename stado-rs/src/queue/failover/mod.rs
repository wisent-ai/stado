//! Single-writer storage with configurable read authority.
//!
//! Mutations commit to the configured primary, then mirror to the read-only
//! disaster-recovery backend. Replica errors are reported without turning an
//! already-committed primary mutation into a false failure. Normal clients may
//! use the backup after a failed primary read; authority-sensitive users retain
//! the same write mirror but return the primary error. A successful `absent`
//! answer is always authoritative. The backup is never promoted to writer.

use std::sync::Arc;

use super::{BlobBackend, StorageError, UPLOAD_PART_MARKER};

mod blob_backend;
#[cfg(test)]
mod mirror_heal;

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
