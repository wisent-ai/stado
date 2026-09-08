//! Priority-marker index (`queue_priority/<inv_prio>-<ts>-<jid>.json`
//! markers so name-ascending sort = priority-desc + FIFO), the `list_jobs`
//! bulk fetch, and [`list_claimable`] — the ordered index walk every
//! scheduler poll goes through.
//!
//! Port of `stado/queue/listing/__init__.py`.
//!
//! Known Python bug (ported as intended): `listing/__init__.py:
//! 120`, `capacity.py:
//! 121` and `leases/__init__.py:
//! 143` reference
//! `store._azure_backend`, an attribute that never exists (the backend
//! handle is `_blob_backend`). The intended behavior is a single backend
//! handle — exactly what `JobStorage` carries here — so the metadata
//! prefilter Python's `list_fitting` chose between backends for is simply
//! always in force in the oldest-first pass of [`list_claimable`], which has
//! one path.
//!
//! # Index ordering rule
//!
//! **A queued job may never be observable without a marker. Whenever a write
//! and a delete both touch one job's index entries, the write goes first.**
//!
//! The two failure modes are not symmetric, and this asymmetry is what fixes
//! the order of every operation in this module:
//!
//! - A *duplicate* or *stale* marker is harmless. It resolves its job from
//!   its body, is deduplicated by job_id, and costs only scan budget — never
//!   a slot in the returned window.
//! - A *missing* marker is fatal. This index is the whole listing strategy
//!   for `queue/`, so an unindexed queued job is invisible to every
//!   scheduler forever while still reporting state `queued`: it is never
//!   claimed, and `queue drain --wait` never terminates.
//!
//! So: create writes the marker before settling the job blob; a re-key
//! writes the new marker before cleaning the superseded one (see
//! [`delete_markers_scanning`]'s `keep`); and the repair sweep in
//! [`migrations::backfill_priority_markers`] is bounded and repeating rather
//! than latched, so an interrupted window is always eventually re-swept.
//!
//! The components mirror the seams this module already carried: `keys`
//! derives a marker name and recognizes one, `markers` writes and removes
//! index entries, `jobs` is the bulk prefix fetch and the id-only walk, and
//! `claimable` is the ordered index walk plus the oldest-first pass behind
//! it. Every name this module used to declare is re-exported here, so
//! `crate::queue::listing::<item>` resolves exactly as before.
//!
//! [`migrations::backfill_priority_markers`]: super::migrations::backfill_priority_markers

mod claimable;
mod jobs;
mod keys;
mod markers;

pub use claimable::{list_claimable, JobScan};
pub use jobs::{list_job_ids, list_jobs};
pub use keys::{is_marker, marker_path, priority_key};
pub use markers::{delete_marker_for, delete_markers_scanning, write_marker};

/// The index prefix. Ordered by name, and the name is the ordering.
pub const MARKER_PREFIX: &str = "queue_priority/";
