//! The idle worker's own pass of the lease reaper over the jobs it finished.
//!
//! [`crate::queue::reaper`] completes a release job whose worker lease
//! expired from its verified receipt and archive, and it is CAS-fenced, so
//! any caller may run it. With the coordinator tick as its only caller, a
//! coordinator that ticks rarely (an old Stado on a memory-starved host)
//! leaves a job that finished every step and uploaded its receipt — its agent
//! restarted onto an installed Stado before writing the terminal state —
//! `building` for hours with its release unpublished.
//!
//! An idle agent therefore asks the reaper about the jobs whose work trees on
//! this host hold a recent receipt, once per lease window. Only those: the
//! fleet-wide pass lists every running job through the object API and did
//! not finish in five minutes from this host. A job whose lease is still
//! live, or that already left `running/`, is left alone by the reaper itself.

use std::sync::atomic::{AtomicI64, Ordering};
use std::time::{Duration, SystemTime};

use crate::providers::local::disk_cleanup::queue_workdirs::{work_root, WORKDIR_PREFIX};
use crate::queue::JobStorage;

use super::super::agent_log;

/// When this process last started a pass, in Unix seconds.
static LAST_PASS: AtomicI64 = AtomicI64::new(0);

/// A receipt older than this belongs to a job the queue has long settled.
const RECENT_RECEIPT: Duration = Duration::from_secs(2 * 24 * 60 * 60);

/// Job ids of this host's work trees that hold a recent `output/receipt.json`.
fn finished_here() -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(work_root()) else {
        return Vec::new();
    };
    let now = SystemTime::now();
    entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            let job_id = name.strip_prefix(WORKDIR_PREFIX)?.to_string();
            let modified = std::fs::metadata(entry.path().join("output/receipt.json"))
                .and_then(|metadata| metadata.modified())
                .ok()?;
            let age = now.duration_since(modified).unwrap_or_default();
            (age <= RECENT_RECEIPT).then_some(job_id)
        })
        .collect()
}

/// Run one reaper pass over this host's finished jobs when the agent holds no
/// slot and no pass started within the last lease window. A failed pass is
/// logged and left to the next window; it never fails the tick.
pub(super) async fn reap_when_idle(store: &JobStorage, idle: bool, log_fn: &mut dyn FnMut(&str)) {
    let window = crate::config::HEARTBEAT_STALE_MINUTES * 60;
    let now = chrono::Utc::now().timestamp();
    if !idle || now - LAST_PASS.load(Ordering::Relaxed) < window {
        return;
    }
    LAST_PASS.store(now, Ordering::Relaxed);
    let jobs = finished_here();
    if jobs.is_empty() {
        return;
    }
    let log = |message: &str| agent_log(&format!("lease reaper: {message}"));
    match crate::queue::reaper::reap_named(store, &jobs, &log).await {
        Ok(summary) => {
            if summary.release_completions + summary.requeued + summary.failed + summary.unreadable
                > 0
            {
                log_fn(&format!(
                    "lease reaper: of {} finished job(s) here, completed {} release job(s) from \
                     their receipts, requeued {}, failed {}, could not read {}",
                    jobs.len(),
                    summary.release_completions,
                    summary.requeued,
                    summary.failed,
                    summary.unreadable
                ));
            }
        }
        Err(error) => log_fn(&format!("lease reaper: pass failed: {error}")),
    }
}
