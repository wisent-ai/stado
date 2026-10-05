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
//! An idle agent therefore asks the reaper, on every idle tick, about the
//! jobs whose work trees on this host hold a receipt and which are still in
//! `running/`. Only those: the fleet-wide pass lists every running job
//! through the object API and did not finish in five minutes from this host.
//! A job seen gone from `running/` is settled for this process and not asked
//! about again; a job whose lease is still promised is left alone by the
//! reaper itself and asked about again next idle tick.

use std::collections::HashSet;
use std::sync::{LazyLock, Mutex};

use crate::providers::local::disk_cleanup::queue_workdirs::{work_root, WORKDIR_PREFIX};
use crate::queue::JobStorage;

use super::super::agent_log;

/// Jobs with a receipt here that this process has seen leave `running/`.
static SETTLED: LazyLock<Mutex<HashSet<String>>> = LazyLock::new(Mutex::default);

/// Job ids of this host's work trees that hold an `output/receipt.json` and
/// are not yet known to have left `running/`.
fn finished_here() -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(work_root()) else {
        return Vec::new();
    };
    let settled = SETTLED
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            let job_id = name.strip_prefix(WORKDIR_PREFIX)?.to_string();
            entry
                .path()
                .join("output/receipt.json")
                .is_file()
                .then_some(job_id)
        })
        .filter(|job_id| !settled.contains(job_id))
        .collect()
}

/// Run one reaper pass over this host's finished jobs that are still in
/// `running/`, when the agent holds no slot. A failed pass is logged and
/// left to the next idle tick; it never fails the tick.
pub(super) async fn reap_when_idle(store: &JobStorage, idle: bool, log_fn: &mut dyn FnMut(&str)) {
    if !idle {
        return;
    }
    let mut jobs = Vec::new();
    for job_id in finished_here() {
        match store
            .backend()
            .exists(&format!("running/{job_id}.json"))
            .await
        {
            Ok(true) => jobs.push(job_id),
            Ok(false) => {
                SETTLED
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .insert(job_id);
            }
            Err(error) => log_fn(&format!(
                "lease reaper: cannot tell whether {job_id} is still running: {error}"
            )),
        }
    }
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
