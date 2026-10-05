//! Whether a running job — and so the VM it runs on — is alive, used by the
//! reapers to avoid destroying productive work.
//!
//! Port of `stado/monitor/heartbeat_guard.py`.
//!
//! The reaper's primary signal is the agent's capacity broadcast in
//! gs://<bucket>/capacity/<consumer_id>.json. When the agent runs a long
//! training subprocess the broadcast loop can starve past the next
//! publication it promised even though the agent process is alive and the
//! training is actively producing checkpoints. Reaping that VM destroys
//! hours of work.
//!
//! This module provides the second signal: each running job's own lease
//! promise, renewed by a task keyed to the job's process and not to the
//! agent's broadcast loop, and whatever the job wrote after that promise
//! (its pulse, its checkpoint shards). If ANY job assigned to a VM is alive
//! by that verdict, the reap is deferred.
//!
//! `heartbeat` holds the verdict, `checkpoint` reads the newest write under
//! a job's checkpoint prefix, `running_refs` re-lists running/ to answer
//! which jids claim a VM ref, and `self_terminating` recognizes the
//! maintenance command whose own success kills the agent. Every name a
//! caller outside this module uses is re-exported here.

use chrono::{DateTime, Utc};

mod checkpoint;
mod heartbeat;
mod running_refs;
mod self_terminating;

pub use heartbeat::{any_job_alive, job_liveness, JobLiveness};
pub use running_refs::{build_ref_to_jids, fresh_jids_pointing_to_ref};
pub use self_terminating::finalize_if_self_terminating;

/// Sentinel returned by [`fresh_jids_pointing_to_ref`] when the running/
/// listing itself fails, so callers defer (treat the VM as in-use).
pub const LIST_FAILED_SENTINEL: &str = "__list_failed__";

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
