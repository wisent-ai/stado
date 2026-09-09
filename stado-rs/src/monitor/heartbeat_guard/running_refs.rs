//! The running/ listing read as a VM-ref question: which jids claim a
//! ref right now, and the whole ref -> jids map one tick needs.

use std::collections::BTreeMap;

use crate::queue::{JobStorage, StorageError};

use super::LIST_FAILED_SENTINEL;

/// List jids whose running/ blob has instance_ref == ref, read FRESH
/// at call time. Used by reap_dead_agents as the FINAL safety check
/// immediately before any delete_instance, to defeat the race that
/// burned restart 16 of job 724084db at 2026-05-17T21:26:07: Branch B
/// (never-worked) checked `instance_ref not in active_refs` where
/// active_refs was a cache built at function entry, list_jobs("running")
/// DID NOT return 724084db at that tick (transient listing miss), the
/// gate held, the VM was deleted, and _requeue_jids_after_reap got an
/// empty jids list (_ref_to_jids came from the same cached listing) —
/// so the job was left wedged in running/ pointing at a deleted VM,
/// auto-recovery delayed until heartbeat staled.
///
/// On read-error, returns a sentinel non-empty list so the caller
/// DEFERS (treats VM as in-use) rather than reaping — same fail-safe
/// philosophy as any_job_heartbeat_fresh.
pub async fn fresh_jids_pointing_to_ref(store: &JobStorage, instance_ref: &str) -> Vec<String> {
    match store.list_jobs("running", 0).await {
        Ok(jobs) => jobs
            .iter()
            .filter(|j| j.instance_ref.as_deref() == Some(instance_ref))
            .map(|j| j.job_id.clone())
            .filter(|jid| !jid.is_empty())
            .collect(),
        Err(_) => vec![LIST_FAILED_SENTINEL.to_string()],
    }
}

/// Build instance_ref -> list[job_id] from store.list_jobs('running').
/// Used by the reaper to find which jobs claim each VM ref before the
/// heartbeat freshness check.
pub async fn build_ref_to_jids(
    store: &JobStorage,
) -> Result<BTreeMap<String, Vec<String>>, StorageError> {
    let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for job in store.list_jobs("running", 0).await? {
        if let Some(instance_ref) = job.instance_ref.filter(|r| !r.is_empty()) {
            if !job.job_id.is_empty() {
                out.entry(instance_ref)
                    .or_default()
                    .push(job.job_id.clone());
            }
        }
    }
    Ok(out)
}
