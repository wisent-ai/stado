//! The running/ listing read as a VM-ref question: which jids claim a
//! ref right now, and the whole ref -> jids map one tick needs.

use std::collections::BTreeMap;

use crate::queue::{JobStorage, StorageError};

use super::LIST_FAILED_SENTINEL;

/// Re-read running jobs pointing at this instance immediately before a reap.
/// A cached listing can miss a new or temporarily unlisted job; it cannot
/// authorize deleting that job's VM.
///
/// A read error returns a non-empty sentinel so callers defer rather than
/// treating an unobserved instance as unused.
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
