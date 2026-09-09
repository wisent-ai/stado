//! The [`BlobBackend`] trait: the backend-neutral blob contract every
//! storage backend in `queue/` implements. Body lifted out of `queue/mod.rs`
//! unchanged.

use std::collections::BTreeMap;
use std::path::Path;

use async_trait::async_trait;
use chrono::{DateTime, Utc};

use super::{BlobInfo, StorageError, VersionedText};

/// Backend-neutral blob contract shared by local filesystem, GCS, S3, and
/// Azure Blob Storage. `path` is always a backend-root-relative name using
/// `/` separators.
#[async_trait]
pub trait BlobBackend: Send + Sync {
    /// The blob name THIS backend addresses one `stado://` object by.
    ///
    /// Two spellings of one object exist in this crate and they are both
    /// plain strings, so every caller holding an [`crate::object_store::
    /// ObjectRef`] has had to guess which one its backend wanted: the
    /// qualified store path `ecosystem/<namespace>/<key>`, which is where a
    /// filesystem or bucket backend keeps the bytes, or the bare `<key>`,
    /// which is what the object API takes because it re-prefixes with its own
    /// configured namespace. Guessing wrong is silent in the worst direction:
    /// a read reports the object absent, and a write creates a second object
    /// at `ecosystem/<ns>/ecosystem/<ns>/<key>` where nothing resolves it.
    /// That is not hypothetical — it happened 417 times to `probierz`, and
    /// `stado storage stat` still answered `absent` for objects the same
    /// store served over HTTP 200.
    ///
    /// So the backend answers it. The default is the qualified path, which is
    /// what every storage backend but one uses.
    fn blob_path(&self, object: &crate::object_store::ObjectRef) -> String {
        object.storage_path()
    }

    /// The listing prefix THIS backend takes for a `stado://<namespace>/`
    /// prefix, which may name a whole namespace and carry no key.
    fn blob_prefix(&self, namespace: &str, prefix: &str) -> Result<String, StorageError> {
        crate::object_store::ObjectRef::namespace_prefix(namespace, prefix)
    }

    /// Unconditional overwrite of a text blob.
    async fn upload_text(&self, path: &str, content: &str) -> Result<(), StorageError>;

    /// Unconditional overwrite of a binary blob (Python
    /// `blob.upload_from_string(bytes)` — box artifact collection).
    async fn upload_bytes(&self, path: &str, content: &[u8]) -> Result<(), StorageError>;

    /// Text content of a blob, or `None` when it does not exist.
    async fn download_text(&self, path: &str) -> Result<Option<String>, StorageError>;

    /// Raw bytes of a blob, or `None` when it does not exist.
    async fn download_bytes(&self, path: &str) -> Result<Option<Vec<u8>>, StorageError>;

    /// One `stado://releases/...` object off the public release channel, for
    /// backends that HAVE such a channel. The plain blob routes answer for
    /// the store's configured namespace, so a cross-namespace release URI
    /// read through them silently becomes `<namespace>/releases/...` and
    /// reports a published artifact absent — which is how every fleet
    /// delivery of stado 0.7.6 failed while the archive sat published. The
    /// default falls back to the namespaced path so disk-backed stores,
    /// which hold releases under their literal storage path, keep working.
    async fn download_release(&self, uri: &str) -> Result<Option<Vec<u8>>, StorageError> {
        let object = crate::object_store::ObjectRef::parse(uri)
            .map_err(|error| StorageError::Other(error.to_string()))?;
        self.download_bytes(&object.storage_path()).await
    }

    /// Download one blob to a local file; `false` when it is absent.
    async fn download_to_filename(&self, path: &str, dest: &Path) -> Result<bool, StorageError>;

    /// Atomically create a text blob; `false` if it already exists
    /// (GCS `ifGenerationMatch=0`, local `O_CREAT|O_EXCL`).
    async fn upload_text_if_absent(&self, path: &str, content: &str) -> Result<bool, StorageError>;

    /// Atomically upload a local file; `false` if the blob exists.
    async fn upload_file_if_absent(
        &self,
        path: &str,
        local_file: &Path,
    ) -> Result<bool, StorageError>;

    /// Read text together with the backend generation/version used for CAS.
    async fn download_text_versioned(
        &self,
        path: &str,
    ) -> Result<Option<VersionedText>, StorageError>;

    /// Replace text iff the current version matches `expected_version`;
    /// returns the new version. [`StorageError::StorageConflict`] when the
    /// race is lost. The empty expected version is rejected by the
    /// [`JobStorage`] facade (Python `ValueError`).
    ///
    /// [`JobStorage`]: crate::queue::JobStorage
    async fn compare_and_swap_text(
        &self,
        path: &str,
        expected_version: &str,
        content: &str,
    ) -> Result<String, StorageError>;

    /// Delete a blob (and its local metadata sidecar). Idempotent.
    async fn delete(&self, path: &str) -> Result<(), StorageError>;

    /// Whether the blob exists.
    async fn exists(&self, path: &str) -> Result<bool, StorageError>;

    /// Blob names under `prefix`. When `oldest_first > 0`, return only that
    /// many names sorted by creation time ascending — bounded listing for
    /// hot prefixes (queue/ has 14k+ blobs).
    async fn list_paths(
        &self,
        prefix: &str,
        oldest_first: usize,
    ) -> Result<Vec<String>, StorageError>;

    /// One ordered page of blob names under `prefix`: name-ascending,
    /// strictly after `start_after`, at most `limit` names (`0` = no cap).
    ///
    /// This is the primitive an ordered-index walk needs and [`Self::list_paths`]
    /// cannot give it. `list_paths` materializes the whole prefix before
    /// anything can be cut, so a scheduler that wants the first few names of a
    /// 14k-blob index pays for all 14k on every poll. Lexicographic order is
    /// the contract rather than an accident, because the priority index encodes
    /// its ordering IN the name; `start_after` is exclusive so a caller can
    /// hand back the last name it saw to resume, and wrap to the head of the
    /// prefix by passing `""`.
    async fn list_page(
        &self,
        prefix: &str,
        start_after: &str,
        limit: usize,
    ) -> Result<Vec<String>, StorageError> {
        // Correct for every backend and cheap for none: the whole prefix is
        // listed and then cut. Backends whose listing API can express "after
        // this name, at most this many" override this with the server-side
        // form; the ones that cannot at least keep the semantics honest.
        let mut names = self.list_paths(prefix, 0).await?;
        names.sort_unstable();
        let cut = names.partition_point(|name| name.as_str() <= start_after);
        names.drain(..cut);
        if limit > 0 {
            names.truncate(limit);
        }
        Ok(names)
    }

    /// Last-modified time of a blob, or `None` when absent.
    async fn updated_at(&self, path: &str) -> Result<Option<DateTime<Utc>>, StorageError>;

    /// Merge string metadata onto an existing blob. No-op when the blob is
    /// absent (local backend semantics; see module docs in `gcs.rs` for the
    /// GCS 404 handling).
    async fn set_metadata(
        &self,
        path: &str,
        kv: &BTreeMap<String, String>,
    ) -> Result<(), StorageError>;

    /// Name, updated-ts and metadata for every blob under `prefix`, so
    /// consumers can filter on metadata before downloading the full body.
    async fn list_blobs_with_meta(&self, prefix: &str) -> Result<Vec<BlobInfo>, StorageError>;
}
