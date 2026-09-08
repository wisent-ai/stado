//! The verdict that separates a genuine listing race from a set of confirmed
//! orphans: the last check standing between a dead VM and its delete, and the
//! one that must not defer forever on a blob that merely exists.

use std::collections::BTreeMap;

use chrono::Utc;

use crate::models::Job;
use crate::monitor::heartbeat_guard as hg;
use crate::queue::JobStorage;

use super::super::{elapsed_seconds, MonitorError};

/// Decide whether a freshly re-listed running/ jid set
/// (fresh_jids_pointing_to_ref) is a GENUINE active_refs race that must
/// defer a reap, vs a set of confirmed ORPHANS safe to reap+requeue.
///
/// fresh_jids_pointing_to_ref's "fresh" means "re-listed at call time"
/// (beats the cached-listing race), NOT "the job is alive" — it returns
/// every running/ blob pointing at the ref with zero liveness check. On
/// a CONFIRMED-dead agent (reaper Branch A: consumer_id absent from live
/// capacity) that made the guard defer on mere blob existence forever:
/// 0db3438b/6a0fceba sat ~3h on agents that were gone (no capacity
/// broadcast, heartbeats ~2.5h stale), never requeued, and the whole
/// gpt-oss-20b queue totally stalled (2026-05-19).
///
/// A real race is only when the job is plausibly alive: the GCS re-list
/// itself failed (fail-safe defer), OR some jid still heartbeats / writes
/// checkpoints fresh, OR some jid started so recently it has not had time
/// to heartbeat yet (boot grace). Otherwise every jid is a stale orphan
/// on a dead VM -> return False so the caller reaps and requeues them.
pub(super) async fn safety_is_real_race(
    store: &JobStorage,
    jids: &[String],
    hb_threshold: f64,
) -> Result<bool, MonitorError> {
    if jids.is_empty() {
        return Ok(false);
    }
    if jids.iter().any(|j| j == hg::LIST_FAILED_SENTINEL) {
        return Ok(true); // GCS re-list failure: never reap on unknown state
    }
    if hg::any_job_heartbeat_fresh(store, jids, hb_threshold).await
        || hg::any_job_checkpoint_fresh_jids(store, jids, 5400.0).await
    {
        return Ok(true);
    }
    // Python does NOT catch list_jobs errors here — propagate via `?`.
    let running: BTreeMap<String, Job> = store
        .list_jobs("running", 0)
        .await?
        .into_iter()
        .map(|j| (j.job_id.clone(), j))
        .collect();
    let now = Utc::now();
    for jid in jids {
        let Some(job) = running.get(jid) else {
            continue;
        };
        let Some(sa) = job.started_at.as_deref().filter(|s| !s.is_empty()) else {
            continue;
        };
        // Python: except (ValueError, TypeError) -> continue.
        let Some(started) = hg::parse_iso_lenient(sa) else {
            continue;
        };
        if elapsed_seconds(now, started) < 1800.0 {
            return Ok(true); // just dispatched, no heartbeat yet (real race)
        }
    }
    Ok(false)
}
