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
//! Known Python bug (ported as INTENDED, not as written): `capacity.py:
//! 121`, `listing/__init__.py:
//! 120` and `leases/__init__.py:
//! 143` reference
//! `store._azure_backend`, an attribute that never exists — the only backend
//! handle on Python `JobStorage` is `_blob_backend`. The intended behavior
//! is a single backend handle, which is exactly what [`JobStorage`] holds.
//!
//! [`control`], [`copy`] and [`reaper`] are the exceptions: they have NO
//! Python original. [`control`] is the fleet pause switch behind `stado queue
//! pause`, the drain gate every storage migration already assumed existed.
//! [`copy`] is the backend-to-backend copier the outage forced (GCS billing
//! closed, queue state has to reach Azure Blob), a direction no Python tool
//! covers. [`reaper`] is the provider-neutral phantom-job reaper the
//! coordinator tick runs so a dead worker's `running/` record and a silent
//! worker's `assigned_to` pin recover even when no cloud monitor arm is
//! configured or reachable. Application credentials belong in the separate Skarbiec service and
//! are not part of queue storage or backend migration.
//!
//! The blob contract itself — [`BlobBackend`], [`BlobInfo`],
//! [`VersionedText`], [`StorageError`], the adapter factory and the
//! Python-compatible JSON serializers — lives in the `contract` component
//! tree beside this file and is re-exported below, so every consumer keeps
//! naming `crate::queue::<name>`.

pub mod azure_blob;
pub mod capacity;
mod contract;
pub mod control;
pub mod copy;
pub(crate) mod failover;
pub mod gcs;
pub mod leases;
pub mod listing;
pub mod local_file;
pub mod migrations;
pub mod reaper;
pub mod runs;
pub mod s3;
pub mod stado_object;
pub mod storage;
pub mod submit;
pub mod tombstone;

/// Canonical queue/storage layout contract recorded in release manifests.
pub const STORAGE_LAYOUT_VERSION: u16 = true as u16;

/// The suffix `put` stages a large body under: `<key>.__stado_upload/<upload
/// id>/<index>`. A part is not an object, and the difference decides whether a
/// key may be listed, composed, or served — so the marker is declared once
/// here rather than spelled again at each reader.
pub const UPLOAD_PART_MARKER: &str = ".__stado_upload/";
pub use azure_blob::AzureBlobBackend;
pub use contract::{BlobBackend, BlobInfo, StorageError, VersionedText};
pub use gcs::GcsBackend;
pub use local_file::LocalBackend;
pub use s3::S3Backend;
pub use stado_object::StadoObjectBackend;
pub use storage::JobStorage;

pub(crate) use contract::{construct_backend, json_str, python_json_dumps, BackendLocator};
