//! Job queue storage layer.
//!
//! Port of `stado/queue/storage.py` (JobStorage facade), `local_file.py`
//! (local filesystem backend), `s3.py` (aws-sdk-s3 backend),
//! `azure_blob.py` (hand-rolled Azure Blob REST backend), the SDK path of
//! the inline GCS backend, `runs/__init__.py`, `tracking/tombstone.py`,
//! `listing/__init__.py` (priority-marker index + bulk/priority/fitting
//! listings), `leases/__init__.py` (fenced provider leases), `capacity.py`
//! (capacity broadcasts), and `migrations.py` (priority-marker backfill).
//!
//! The Python code routes every storage operation through a shared
//! blob-backend contract (Azure/local) or the GCS SDK. Here that contract
//! is the [`BlobBackend`] async trait, consumed as `Arc<dyn BlobBackend>`.
//!
//! Known Python bug (ported as INTENDED, not as written): `capacity.py:121`,
//! `listing/__init__.py:120` and `leases/__init__.py:143` reference
//! `store._azure_backend`, an attribute that never exists — the only backend
//! handle on Python `JobStorage` is `_blob_backend`. The intended behavior
//! is a single backend handle, which is exactly what [`JobStorage`] holds.
//!
//! [`control`] and [`copy`] are the exceptions: they have NO Python
//! original. [`control`] is the fleet pause switch behind `stado queue
//! pause`, the drain gate every storage migration already assumed existed.
//! [`copy`] is the backend-to-backend copier the outage forced (GCS billing
//! closed, queue state has to reach Azure Blob), a direction no Python tool
//! covers. Application credentials belong in the separate Skarbiec service and
//! are not part of queue storage or backend migration.

pub mod azure_blob;
pub mod capacity;
pub mod control;
pub mod copy;
pub(crate) mod failover;
pub mod gcs;
pub mod leases;
pub mod listing;
pub mod local_file;
pub mod migrations;
pub mod runs;
pub mod s3;
#[cfg(test)]
pub mod secrets;
pub mod storage;
pub mod submit;
pub mod tombstone;

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};

/// Canonical queue/storage layout contract recorded in release manifests.
pub const STORAGE_LAYOUT_VERSION: u16 = true as u16;
pub use azure_blob::AzureBlobBackend;
pub use gcs::GcsBackend;
pub use local_file::LocalBackend;
pub use s3::S3Backend;
pub use storage::JobStorage;

use crate::capabilities::StorageAdapter;

/// Concrete locators consumed by the single storage-adapter factory.
///
/// The catalog selects the adapter; callers supply only endpoint values. This
/// keeps constructor ownership here instead of duplicating backend-name
/// switches in the queue facade, copier, doctor, and recovery commands.
pub(crate) struct BackendLocator<'a> {
    pub bucket: &'a str,
    pub account: &'a str,
    pub container: &'a str,
    pub region: &'a str,
    pub path: &'a str,
}

pub(crate) async fn construct_backend(
    adapter: StorageAdapter,
    locator: BackendLocator<'_>,
) -> Result<Arc<dyn BlobBackend>, StorageError> {
    match adapter {
        StorageAdapter::Gcs => Ok(Arc::new(GcsBackend::new(locator.bucket).await?)),
        StorageAdapter::AzureBlob => Ok(Arc::new(AzureBlobBackend::new(
            locator.account,
            locator.container,
        )?)),
        StorageAdapter::S3 => Ok(Arc::new(
            S3Backend::new(locator.bucket, locator.region).await?,
        )),
        StorageAdapter::Local => Ok(Arc::new(LocalBackend::new(locator.path)?)),
    }
}

/// Text blob content with its opaque backend version token for
/// compare-and-swap.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VersionedText {
    pub content: String,
    pub version: String,
}

/// Backend-agnostic blob descriptor used to filter on metadata before
/// downloading a body. Callers retain the backend separately.
#[derive(Debug, Clone)]
pub struct BlobInfo {
    pub name: String,
    pub updated: Option<DateTime<Utc>>,
    pub size: Option<u64>,
    pub metadata: BTreeMap<String, String>,
}

/// Storage-layer failures distinguish lost conditional-write races, missing
/// objects, invalid local paths, authentication, provider API, transport, and
/// serialization failures.
#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    /// A conditional write lost a race with another writer.
    #[error("{0}")]
    StorageConflict(String),
    /// A conditional operation required a blob that does not exist.
    #[error("blob not found: {0}")]
    NotFound(String),
    /// A local backend path escaped the deployment root.
    #[error("storage path escapes deployment root: {0}")]
    PathEscape(String),
    /// GCS JSON API returned a non-success status other than 404/412.
    #[error("GCS API error HTTP {status}: {body}")]
    Gcs { status: u16, body: String },
    /// GCP authentication could not be established (no gsutil fallback).
    #[error("GCP authentication failed: {0}")]
    Auth(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Http(#[from] reqwest::Error),
    #[error("{0}")]
    Other(String),
}

/// Backend-neutral blob contract shared by local filesystem, GCS, S3, and
/// Azure Blob Storage. `path` is always a backend-root-relative name using
/// `/` separators.
#[async_trait]
pub trait BlobBackend: Send + Sync {
    /// Unconditional overwrite of a text blob.
    async fn upload_text(&self, path: &str, content: &str) -> Result<(), StorageError>;

    /// Unconditional overwrite of a binary blob (Python
    /// `blob.upload_from_string(bytes)` — box artifact collection).
    async fn upload_bytes(&self, path: &str, content: &[u8]) -> Result<(), StorageError>;

    /// Text content of a blob, or `None` when it does not exist.
    async fn download_text(&self, path: &str) -> Result<Option<String>, StorageError>;

    /// Raw bytes of a blob, or `None` when it does not exist.
    async fn download_bytes(&self, path: &str) -> Result<Option<Vec<u8>>, StorageError>;

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

/// Serialize a string as a JSON string literal. Used to build the small
/// fixed-shape JSON bodies (priority markers, tombstones, metadata
/// sidecars) with Python `json.dumps` default separators (", " / ": ").
pub(crate) fn json_str(s: &str) -> String {
    serde_json::to_string(s).expect("string serialization is infallible")
}

/// Serialize a JSON value byte-compatibly with Python `json.dumps(value)`:
/// default separators (", " between items, ": " after keys) and
/// `ensure_ascii=True` escaping. Used for the capacity broadcasts and the
/// migration sentinel, which Python readers parse with `json.loads`.
pub(crate) fn python_json_dumps(value: &serde_json::Value) -> Result<String, serde_json::Error> {
    use serde::Serialize;

    /// serde_json `Formatter` reproducing CPython's default `json.dumps`
    /// separators.
    struct PythonSeparators;

    impl serde_json::ser::Formatter for PythonSeparators {
        fn begin_object_key<W>(&mut self, writer: &mut W, first: bool) -> std::io::Result<()>
        where
            W: ?Sized + std::io::Write,
        {
            if first {
                Ok(())
            } else {
                writer.write_all(b", ")
            }
        }

        fn begin_object_value<W>(&mut self, writer: &mut W) -> std::io::Result<()>
        where
            W: ?Sized + std::io::Write,
        {
            writer.write_all(b": ")
        }

        fn begin_array_value<W>(&mut self, writer: &mut W, first: bool) -> std::io::Result<()>
        where
            W: ?Sized + std::io::Write,
        {
            if first {
                Ok(())
            } else {
                writer.write_all(b", ")
            }
        }
    }

    let mut buf = Vec::new();
    let mut serializer = serde_json::Serializer::with_formatter(&mut buf, PythonSeparators);
    value.serialize(&mut serializer)?;
    Ok(crate::models::ensure_ascii(
        &String::from_utf8(buf).expect("serde_json emits UTF-8"),
    ))
}
