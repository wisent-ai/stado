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
//! [`copy`] copies queue state between supported storage backends.
//! [`reaper`] is the provider-neutral phantom-job reaper the
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

/// The suffix `put` stages a large body under: `<key>.__stado_upload/<upload
/// id>/<index>`. A part is not an object, and the difference decides whether a
/// key may be listed, composed, or served — so the marker is declared once
/// here rather than spelled again at each reader.
pub const UPLOAD_PART_MARKER: &str = ".__stado_upload/";

/// The name of one staged part under `<key>.__stado_upload/<upload id>/`:
/// its index zero-padded to a fixed width, so parts list in order. Writers
/// (`put`, the S3 multipart stage) and composition share this one spelling;
/// the deletion authorizer also reads the earlier one
/// ([`is_staged_part_name`]).
pub fn upload_part_name(index: u32) -> String {
    format!("{index:08}")
}

/// Whether `name` is exactly the name [`upload_part_name`] gives some index:
/// digits only, at the padded width, so a staged key is recognised by the
/// shape its writer gave it and not by any looser pattern.
pub fn is_upload_part_name(name: &str) -> bool {
    match name.parse::<u32>() {
        Ok(index) => upload_part_name(index) == name,
        Err(_not_an_index) => false,
    }
}

/// Whether `name` is a name the chunked uploader has given a staged part:
/// [`upload_part_name`]'s, or the one it gave before it used that spelling,
/// the index zero-padded only to the width of the part count (`000` of 514
/// parts). Composition reads only the current spelling; removal reads both,
/// so the parts of an upload abandoned under the earlier names can still be
/// discarded through the product.
pub fn is_staged_part_name(name: &str) -> bool {
    is_upload_part_name(name)
        || (!name.is_empty()
            && name.len() < upload_part_name(0).len()
            && name.bytes().all(|byte| byte.is_ascii_digit()))
}
pub use azure_blob::AzureBlobBackend;
pub use contract::{BlobBackend, BlobInfo, ChangeWatch, Listing, StorageError, VersionedText};
pub use gcs::GcsBackend;
pub use local_file::LocalBackend;
pub use s3::{S3Backend, S3CompatibleLocator};
pub use stado_object::StadoObjectBackend;
pub use storage::{JobStorage, ServerStorage};

pub(crate) use contract::{construct_backend, json_str, python_json_dumps, BackendLocator};
