//! The idle worker's own pass of the lease reaper.
//!
//! [`crate::queue::reaper::reap_expired_leases`] completes a release job
//! whose worker lease expired from its verified receipt and archive, and it
//! is CAS-fenced, so any caller may run it. Its only caller was the
//! coordinator tick, and on 2026-09-28 the fleet's coordinator ran an old
//! Stado on a memory-starved host and ticked about twice an hour: the darwin
//! job of stado 0.22.17 finished every step and uploaded its receipt, its
//! agent restarted onto an installed Stado before writing the terminal state,
//! and the build stayed `building` for hours with its release unpublished.
//!
//! An idle agent therefore runs the same pass once per lease window. Idle,
//! because a pass lists the fleet's running jobs and a busy host has work of
//! its own; once per window, because nothing can expire sooner.

use std::sync::atomic::{AtomicI64, Ordering};
use std::time::Duration;

use crate::queue::JobStorage;

use super::super::agent_log;

/// When this process last started a pass, in Unix seconds.
static LAST_PASS: AtomicI64 = AtomicI64::new(0);

/// Run one reaper pass when the agent holds no slot and no pass started
/// within the last lease window, bounded by `budget`. A failed or slow pass
/// is logged and left to the next window; it never fails the tick.
pub(super) async fn reap_when_idle(
    store: &JobStorage,
    idle: bool,
    budget: Duration,
    log_fn: &mut dyn FnMut(&str),
) {
    let window = crate::config::HEARTBEAT_STALE_MINUTES * 60;
    let now = chrono::Utc::now().timestamp();
    if !idle || now - LAST_PASS.load(Ordering::Relaxed) < window {
        return;
    }
    LAST_PASS.store(now, Ordering::Relaxed);
    let log = |message: &str| agent_log(&format!("lease reaper: {message}"));
    match tokio::time::timeout(
        budget,
        crate::queue::reaper::reap_expired_leases(store, &log),
    )
    .await
    {
        Ok(Ok(summary)) => {
            if summary.release_completions + summary.requeued + summary.failed > 0 {
                log_fn(&format!(
                    "lease reaper: completed {} release job(s) from their receipts, requeued {}, \
                     failed {}",
                    summary.release_completions, summary.requeued, summary.failed
                ));
            }
        }
        Ok(Err(error)) => log_fn(&format!("lease reaper: pass failed: {error}")),
        Err(_) => log_fn(&format!(
            "lease reaper: pass did not finish within {}s; the next lease window retries it",
            budget.as_secs()
        )),
    }
}
