//! The per-tick index of lifecycle residue: which jobs still have a blob,
//! a status entry or a companion that a retained run's cleanup owes.

use serde_json::Value;
use std::collections::HashSet;

use crate::queue::runs::ALL_PREFIXES;
use crate::queue::{JobStorage, StorageError};

/// Build one lifecycle-residue index per tick. Exact lifecycle/status paths
/// encode the job id; priority and transition companions state it in their
/// JSON body, so they are read once here rather than searched once per run.
pub(super) async fn cleanup_residue_job_ids(
    store: &JobStorage,
) -> Result<HashSet<String>, StorageError> {
    let mut job_ids = HashSet::new();
    for prefix in ALL_PREFIXES {
        let start = format!("{prefix}/");
        for path in store.list_paths(&start, 0).await? {
            if let Some(job_id) = path
                .strip_prefix(&start)
                .and_then(|tail| tail.strip_suffix(".json"))
                .filter(|tail| !tail.is_empty() && !tail.contains('/'))
            {
                job_ids.insert(job_id.to_string());
            }
        }
    }
    for path in store.list_paths("status/", 0).await? {
        if let Some(job_id) = path
            .strip_prefix("status/")
            .and_then(|tail| tail.split('/').next())
            .filter(|job_id| !job_id.is_empty())
        {
            job_ids.insert(job_id.to_string());
        }
    }
    for prefix in ["queue_priority/", "job-transitions/"] {
        for path in store.list_paths(prefix, 0).await? {
            if prefix == "queue_priority/" && !crate::queue::listing::is_marker(&path) {
                continue;
            }
            let Some(body) = store.download_text(&path).await? else {
                continue;
            };
            let document: Value = serde_json::from_str(&body).map_err(|error| {
                StorageError::Other(format!("invalid lifecycle companion {path}: {error}"))
            })?;
            let job_id = document
                .get("job_id")
                .and_then(Value::as_str)
                .filter(|job_id| !job_id.is_empty())
                .ok_or_else(|| {
                    StorageError::Other(format!("lifecycle companion {path} has no job_id"))
                })?;
            if prefix == "job-transitions/"
                && document
                    .get("state")
                    .and_then(Value::as_str)
                    .is_some_and(crate::queue::storage::transition_is_retired)
            {
                continue;
            }
            job_ids.insert(job_id.to_string());
        }
    }
    Ok(job_ids)
}

pub(super) fn retained_run_has_residue(
    residue_job_ids: &HashSet<String>,
    job_ids: &[String],
) -> bool {
    job_ids
        .iter()
        .any(|job_id| residue_job_ids.contains(job_id))
}
