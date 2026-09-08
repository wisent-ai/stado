//! Monitor running jobs: check heartbeat, status, cleanup.
//!
//! Port of `stado/monitor/monitor.py` (`check_running_jobs` +
//! `reap_dead_agents`) and `stado/monitor/reap/helpers.py` (the
//! monitor-internal requeue/completed-ref helpers).
//!
//! Handles four exit conditions for a running job:
//!   COMPLETED         -> finalize success path
//!   FAILED            -> finalize failure path + alert
//!   preempted (Spot)  -> instance is TERMINATED but the Job is otherwise healthy.
//!                       Delete the GCE instance, increment preempt_count, requeue.
//!                       preempt_count is separate from restarts so a Spot-heavy
//!                       job doesn't burn the restart budget on preemptions alone.
//!   instance gone OR
//!   stale heartbeat   -> requeue (counted against restarts).
//!
//! The components are the passes this file already ran: [`running`] holds the
//! per-running-job pass plus the ghost-VM delete it performs, [`reaper`] the
//! dead / never-worked / wedged VM pass plus the completed-refs scan and the
//! race verdict that defer it, and [`lifecycle`] the requeue moves both
//! passes reach for. The error type, the log sink and the Python-parity
//! renderers stay here, so `crate::monitor::monitor::<item>` resolves exactly
//! as before.

// ---------------------------------------------------------------------------
// reap/helpers.py ports (monitor-internal)
// ---------------------------------------------------------------------------
// lifecycle, plus running::vm_delete, reaper::completions and reaper::race —
// each helper sits next to the pass that calls it.
mod lifecycle;

// ---------------------------------------------------------------------------
// monitor.py ports
// ---------------------------------------------------------------------------
// running::pass and reaper::sweep.
mod reaper;
mod running;

use std::collections::HashSet;

use chrono::{DateTime, Utc};

use crate::providers::ProviderError;
use crate::queue::StorageError;

pub use reaper::reap_dead_agents;
pub use running::check_running_jobs;

/// Monitor-layer error. Python lets storage/provider exceptions propagate
/// out of the per-tick functions; both source layers map onto one error so
/// `?` does the same.
#[derive(Debug, thiserror::Error)]
pub enum MonitorError {
    /// Storage (queue/blob) failures.
    #[error(transparent)]
    Storage(#[from] StorageError),
    /// Provider (GCE/AWS/Azure API) failures.
    #[error(transparent)]
    Provider(#[from] ProviderError),
}

/// Python `_log`: stderr with the [monitor] prefix.
pub(crate) fn log(msg: &str) {
    eprintln!("[monitor] {msg}");
}

/// `(now - then)` in float seconds — Python `timedelta.total_seconds()`.
fn elapsed_seconds(now: DateTime<Utc>, then: DateTime<Utc>) -> f64 {
    (now - then).num_milliseconds() as f64 / 1000.0
}

/// Python repr of a list of strings (`['a', 'b']`) for log-line parity
/// with the Cloud Function logs operators grep.
fn py_str_list(items: &[String]) -> String {
    let inner: Vec<String> = items.iter().map(|s| format!("'{s}'")).collect();
    format!("[{}]", inner.join(", "))
}

/// Python `list(dict.fromkeys(items))`: dedup preserving first-seen order.
fn dedup_preserve_order(items: Vec<String>) -> Vec<String> {
    let mut seen = HashSet::new();
    items
        .into_iter()
        .filter(|i| seen.insert(i.clone()))
        .collect()
}
