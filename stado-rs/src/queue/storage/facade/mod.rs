//! The [`JobStorage`] type itself: its fields, its constructors
//! ([`construct`]), its storage-layout marker ([`layout`]), its explicit
//! handle builders and accessors ([`handle`]) and its thin delegates to the
//! persistence backend ([`blobs`]).

use std::sync::Arc;

use crate::queue::BlobBackend;

// `super::copy`, `super::failover` and `super::STORAGE_LAYOUT_VERSION` for
// the components below, which name them exactly as this module's body did.
use crate::queue::{copy, failover, STORAGE_LAYOUT_VERSION};

mod blobs;
mod construct;
mod handle;
mod layout;

/// Job-level storage facade over a [`BlobBackend`]. Cheap to clone.
#[derive(Clone)]
pub struct JobStorage {
    pub(in crate::queue::storage) backend: Arc<dyn BlobBackend>,
    backend_name: String,
    bucket_name: String,
    local_path: Option<Arc<str>>,
    backup_endpoint: Option<Arc<super::copy::Endpoint>>,
    /// Where the last bounded claimable-scan stopped in the priority index.
    ///
    /// Reachability, and nothing else. A budgeted scan that always restarted
    /// at the head of the index re-read the same head every poll and could
    /// never see a job sitting past the budget, which is the starvation the
    /// budget was supposed to bound rather than cause. Resuming from the last
    /// visited marker and wrapping at the end of the prefix means every queued
    /// job is reached within a bounded number of polls.
    ///
    /// In memory on purpose: it is a fairness hint, not a fact about the
    /// queue. Persisting it would add a document that has to be written on
    /// every poll and reconciled after every crash, to protect a value whose
    /// only failure mode is starting a scan one page early. Shared across
    /// clones because the clones are one worker's handle on one store.
    scan_cursor: Arc<std::sync::Mutex<String>>,
}
