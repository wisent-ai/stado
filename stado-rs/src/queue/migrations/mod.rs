//! Standing priority-marker repair for queued jobs missing an index entry.
//!
//! Port of `stado/queue/migrations.py`, widened past its original job.
//!
//! The priority-marker index introduced in 0.4.26 is populated by durable
//! queue admission. Jobs already sitting in queue/ when 0.4.26 deployed have
//! no marker, and so does any job whose marker write failed to land after it.
//! Either way the job is stranded: the listing walk IS the index, so an
//! unindexed queued job is never claimed while it still reports `queued`.
//!
//! This module fixes that continuously. The standing bounded repair,
//! [`backfill_priority_markers`], is driven from the coordinator tick by
//! [`crate::queue::reaper::reap_expired_leases`]. It is also driven from
//! [`super::listing::list_claimable`], but ONLY while the whole-prefix
//! pass is still live: a poll asks the cheap [`has_swept`] first, so a
//! store whose index is already covered pays one small read per poll rather
//! than a sweep. It is resumable via a queue_priority/.migration.json
//! sentinel recording (cursor, done, coverage). Each call processes at most
//! `batch` blobs, to fit comfortably inside a Cloud Function 60s tick, and
//! resumes past the recorded cursor.
//!
//! The cursor WRAPS: reaching the end of queue/ rewinds it to the head so
//! the next call sweeps again. `done` no longer terminates the pass — it
//! only records that one full sweep has happened, which retires the
//! whole-prefix listing pass. A repair that latched shut could not see a
//! marker lost after it completed, and nothing else would ever look.
//!
//! The components mirror the seams this module already carried: `budgets`
//! holds the per-call bounds and the shared bulk fan-out, `sentinel` is the
//! resumable (cursor, done, coverage) record and the cheap coverage read,
//! `backfill` is the sweep that writes the marker a queued job is missing,
//! and `prune` is the bounded deletion of entries naming no queued job.
//! Every name this module used to declare is re-exported here, so
//! `crate::queue::migrations::<item>` resolves exactly as before.

mod backfill;
mod budgets;
mod prune;
mod sentinel;

pub use backfill::backfill_priority_markers;
pub use budgets::{BACKFILL_BATCH, MARKER_PRUNE_PER_CALL};
pub use sentinel::{has_swept, SENTINEL_PATH};

pub(crate) use budgets::BULK_WORKERS;
