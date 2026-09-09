//! Per-job heartbeat freshness check used by the reaper to avoid
//! destroying productive VMs.
//!
//! Port of `stado/monitor/heartbeat_guard.py`.
//!
//! The reaper's primary signal is the agent's capacity broadcast in
//! gs://<bucket>/capacity/<consumer_id>.json. When the agent runs a
//! long training subprocess the broadcast loop can starve past
//! CAPACITY_STALE_SECONDS even though the agent process is alive and
//! the training is actively producing checkpoints. Reaping that VM
//! destroys hours of work and forces the job to restart from the last
//! checkpoint (or step 0 if no checkpoints exist).
//!
//! This module provides a second signal — the per-job heartbeat at
//! gs://<bucket>/status/<job_id>/heartbeat — that is written by the
//! running job itself (via the agent's status-watchdog cron) and is
//! NOT coupled to the agent's broadcast loop. If ANY job assigned to
//! a VM has a fresh heartbeat, the agent is alive and the reap is
//! deferred.
//!
//! The components are the signals this guard already kept apart:
//! `heartbeat` reads the per-job heartbeat blob and the timestamp
//! embedded in it, `checkpoint` reads the newest blob under a job's
//! checkpoint prefix — the proof-of-life immune to the very upload that
//! starves the heartbeat — `running_refs` re-lists running/ to answer
//! which jids claim a VM ref, and `self_terminating` recognizes the
//! maintenance command whose own success kills the agent. The clock and
//! the lenient timestamp read all four are spelled against stay here,
//! next to the list-failure sentinel the reaper compares against. Every
//! name a caller outside this module uses is re-exported here, so
//! `crate::monitor::heartbeat_guard::<item>` resolves exactly as before.

use chrono::{DateTime, Utc};

mod checkpoint;
mod heartbeat;
mod running_refs;
mod self_terminating;

pub use checkpoint::{any_job_checkpoint_fresh, any_job_checkpoint_fresh_jids};
pub use heartbeat::any_job_heartbeat_fresh;
pub use running_refs::{build_ref_to_jids, fresh_jids_pointing_to_ref};
pub use self_terminating::{finalize_if_self_terminating, is_self_terminating_command};

/// Sentinel returned by [`fresh_jids_pointing_to_ref`] when the running/
/// listing itself fails, so callers defer (treat the VM as in-use).
pub const LIST_FAILED_SENTINEL: &str = "__list_failed__";

/// Current time as unix seconds (float, microsecond precision) — the
/// Python `time.time()` slot.
fn now_unix() -> f64 {
    unix_seconds(Utc::now())
}

/// `datetime.timestamp()` equivalent: seconds + microseconds fraction.
fn unix_seconds(dt: DateTime<Utc>) -> f64 {
    dt.timestamp() as f64 + f64::from(dt.timestamp_subsec_micros()) / 1e6
}

/// Lenient ISO-8601 → UTC parse used for job.started_at / diag timestamps.
/// Job timestamps are Strings produced by Python `datetime.isoformat()` /
/// Rust `Utc::to_rfc3339()`; parse RFC3339 first, then accept a naive
/// `YYYY-MM-DD[T ]HH:MM:SS[.f]` read assumed UTC (Python `fromisoformat`
/// accepts both separators, and every writer here stamps UTC, so
/// assume-UTC matches production data). Returns None on unparseable input,
/// which callers treat per the Python except-branches they port.
pub(crate) fn parse_iso_lenient(s: &str) -> Option<DateTime<Utc>> {
    if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
        return Some(dt.with_timezone(&Utc));
    }
    for fmt in ["%Y-%m-%dT%H:%M:%S%.f", "%Y-%m-%d %H:%M:%S%.f"] {
        if let Ok(naive) = chrono::NaiveDateTime::parse_from_str(s, fmt) {
            return Some(naive.and_utc());
        }
    }
    None
}
