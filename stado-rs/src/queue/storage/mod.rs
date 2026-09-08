//! JobStorage facade: backend selection + job-level operations.
//!
//! Port of `stado/queue/storage.py::JobStorage`. The Python class routes
//! through `_blob_backend` (Azure/local) or the GCS SDK; here every
//! operation goes through a single `Arc<dyn BlobBackend>` — see the module
//! docs in `queue/mod.rs` for the `_azure_backend` divergence note.
//!
//! All four Python backends are wired: "local" / "gcs" / "azure" / "s3".
//!
//! The components mirror the sections this file already carried: [`facade`]
//! holds the [`JobStorage`] type, its constructors and its thin delegates to
//! the persistence backend; [`records`] holds the durable job records and
//! their serialization; [`lifecycle`] holds the claim, lease and durable
//! lifecycle-transition operations; [`queries`] holds the listing, script and
//! status reads. Every name this module used to declare is re-exported here,
//! so `crate::queue::storage::<item>` resolves exactly as before.

mod facade;
mod lifecycle;
mod queries;
mod records;

// `super::copy` for the moved struct field and accessor that name
// `super::copy::Endpoint` verbatim in `facade`.
use crate::queue::copy;

pub use facade::JobStorage;
pub(crate) use records::{
    is_transition_sentinel_state, transition_is_retired, transition_path,
    validate_cancellation_snapshot, validate_transition_snapshot, WorkdirJobState,
    TRANSITION_RETIRED_STATE,
};

#[derive(Debug, Clone)]
pub(crate) struct TransitionSnapshotProof {
    pub job_id: String,
    pub retired: bool,
}
