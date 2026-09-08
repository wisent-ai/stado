//! By-run reaper: removes per-job cruft once a run is fully terminal.
//!
//! Port of `stado/monitor/reap/run_reaper.py`.
//!
//! A run is reapable when every durable entry has reached a terminal prefix.
//! Reaping first CAS-retains each exact terminal job in the manifest and marks
//! entries reaped; only that committed snapshot permits lifecycle blob
//! deletion.
//!
//! Retention and cleanup are two separate durable facts, because a crash sits
//! between them. [`REAPED_AT`](manifest::REAPED_AT) says the outcomes are
//! retained; only [`CLEANUP_COMPLETED_AT`](manifest::CLEANUP_COMPLETED_AT)
//! says every lifecycle blob and status entry is gone. A manifest carrying
//! the first without the second is a legal state — the run-manifest schema
//! admits both keys — and it re-enters a deletion-only pass that retains
//! nothing again, rewrites no `reaped_at`, and tolerates blobs that are
//! already deleted.

mod manifest;
mod reads;
mod retire;
mod snapshot;

pub use retire::reap_terminal_runs;
pub(crate) use snapshot::{classify_reconciliation_snapshot, validate_reconciliation_final_state};

/// A run retained because its durable schema cannot support destructive reap.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReapRefusal {
    pub run_id: String,
    pub reason: &'static str,
}

/// Python summary dict {"reaped_runs", "deleted_jobs", "examined_runs"}.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReapSummary {
    pub reaped_runs: i64,
    pub deleted_jobs: i64,
    pub examined_runs: i64,
    pub refused_runs: Vec<ReapRefusal>,
}
