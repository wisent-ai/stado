//! The verdict that separates a genuine listing race from a set of confirmed
//! orphans: the last check standing between a dead VM and its delete, and the
//! one that must not defer forever on a blob that merely exists.

use chrono::Utc;

use crate::monitor::heartbeat_guard as hg;
use crate::queue::JobStorage;

/// Decide whether a freshly re-listed running/ jid set
/// (fresh_jids_pointing_to_ref) is a GENUINE active_refs race that must
/// defer a reap, vs a set of confirmed ORPHANS safe to reap+requeue.
///
/// A fresh listing proves only that a running record still exists, not that
/// its job is alive. On a dead agent, record existence alone must not defer
/// reaping forever.
///
/// A real race is only when a job is plausibly alive: the re-list itself
/// failed (fail-safe defer), OR some jid is alive by its worker's promise —
/// which covers a job just claimed, since its claim wrote that promise — or
/// wrote a pulse or checkpoint after it. Otherwise every jid is a stale
/// orphan on a dead VM -> false, so the caller reaps and requeues them.
pub(super) async fn safety_is_real_race(store: &JobStorage, jids: &[String]) -> bool {
    if jids.is_empty() {
        return false;
    }
    if jids.iter().any(|j| j == hg::LIST_FAILED_SENTINEL) {
        return true; // re-list failure: never reap on unknown state
    }
    hg::any_job_alive(store, jids, Utc::now()).await
}
