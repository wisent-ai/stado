//! The deletion pass of a retained run: every lifecycle blob and status
//! entry goes, and only a fully successful pass records that it finished.

use chrono::Utc;
use serde_json::Value;

use crate::monitor::reap::manifest::{
    has_complete_retained_outcomes, py_truthy, CLEANUP_COMPLETED_AT,
};
use crate::queue::runs::{RUN_PREFIX, TERMINAL_PREFIXES};
use crate::queue::{JobStorage, StorageError};

/// Delete every blob under status/<job_id>/. One failing delete does not
/// abandon the rest: the whole directory has to go before cleanup can be
/// recorded, so the pass deletes what it can and reports the first failure for
/// the next pass to resume.
async fn delete_status_dir(store: &JobStorage, job_id: &str) -> Result<(), StorageError> {
    let mut failure = None;
    for path in store.list_paths(&format!("status/{job_id}/"), 0).await? {
        if let Err(error) = store.delete_blob(&path).await {
            failure = failure.or(Some(error));
        }
    }
    match failure {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

/// Delete every lifecycle blob and status entry of a retained run, then record
/// that the cleanup finished. Idempotent: a blob another pass already removed
/// is simply absent, and the completion marker is written only once every
/// deletion of this pass succeeded, so an interrupted sweep is resumed rather
/// than abandoned. Returns how many blobs this pass deleted.
pub(super) async fn sweep_retained_run(
    store: &JobStorage,
    run_id: &str,
    job_ids: &[String],
) -> Result<i64, StorageError> {
    let path = format!("{RUN_PREFIX}/{run_id}.json");
    let Some(retained) = store.read_text_versioned(&path).await? else {
        return Ok(0);
    };
    let retained_manifest: Value = serde_json::from_str(&retained.content)?;
    crate::queue::submit::validate_stored_run_manifest(&retained_manifest, run_id)
        .map_err(|error| StorageError::Other(error.to_string()))?;
    if !has_complete_retained_outcomes(&retained_manifest) {
        return Err(StorageError::Other(format!(
            "run {run_id} cannot be swept without complete retained terminal outcomes"
        )));
    }
    let mut deleted = 0;
    for job_id in job_ids {
        // A prepared transition is a lifecycle companion, not an independent
        // source of truth. Resolve it through the canonical transition
        // protocol against the retained terminal destination before removing
        // any stale projection; never patch a job document here.
        store.recover_job_transition(job_id).await?;
        for prefix in TERMINAL_PREFIXES
            .iter()
            .copied()
            .chain(["queue", "running"])
        {
            let blob = format!("{prefix}/{job_id}.json");
            if store.backend().exists(&blob).await? {
                store.delete_job(prefix, job_id).await?;
                deleted += 1;
            }
        }
        store.repair_priority_markers(job_id, None).await?;
        delete_status_dir(store, job_id).await?;
    }
    for _ in 0..16 {
        let Some(versioned) = store.read_text_versioned(&path).await? else {
            return Ok(deleted);
        };
        let mut manifest: Value = serde_json::from_str(&versioned.content)?;
        let object = manifest.as_object_mut().ok_or_else(|| {
            StorageError::Other(format!("run manifest {run_id} is not a JSON object"))
        })?;
        if object.get(CLEANUP_COMPLETED_AT).is_some_and(py_truthy) {
            return Ok(deleted);
        }
        object.insert(
            CLEANUP_COMPLETED_AT.into(),
            Value::from(Utc::now().format("%Y-%m-%dT%H:%M:%S+00:00").to_string()),
        );
        match store
            .compare_and_swap_text(
                &path,
                &versioned.version,
                &serde_json::to_string_pretty(&manifest)?,
            )
            .await
        {
            Ok(_) => return Ok(deleted),
            Err(StorageError::StorageConflict(_)) => continue,
            Err(StorageError::NotFound(_)) => return Ok(deleted),
            Err(error) => return Err(error),
        }
    }
    Err(StorageError::StorageConflict(format!(
        "run manifest {run_id} remained contended while recording cleanup completion"
    )))
}
