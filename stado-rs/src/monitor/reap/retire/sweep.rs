//! The deletion pass of a retained run: every lifecycle blob and status
//! entry goes, and only a fully successful pass records that it finished.

use serde_json::Value;

use crate::monitor::reap::manifest::{
    has_complete_retained_outcomes, marked, stamp, CLEANUP_COMPLETED_AT,
};
use crate::queue::runs::{RUN_PREFIX, TERMINAL_PREFIXES};
use crate::queue::{JobStorage, StorageError};

/// Delete every blob under status/<job_id>/. One failing delete does not
/// abandon the rest: the whole directory has to go before cleanup can be
/// recorded, so the pass deletes what it can and reports the first failure for
/// the next pass to resume.
async fn delete_status_dir(store: &JobStorage, job_id: &str) -> Result<(), StorageError> {
    let mut failure = None;
    for path in store
        .list_paths(&format!("status/{job_id}/"), crate::queue::Listing::Every)
        .await?
    {
        if let Err(error) = store.delete_blob(&path).await {
            failure = failure.or(Some(error));
        }
    }
    match failure {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

/// Whether `run_id` is a release build's submission run: its jobs' output
/// (`status/<job>/output/`: receipt, archive, log) is the build's evidence
/// and the bytes a release publishes, read on the release's own cadence
/// after the job ended. The sweep retires such a job's queue records and
/// leaves its output to the build that names it.
fn release_build_submission(run_id: &str) -> bool {
    run_id.starts_with(&format!(
        "run-{}-",
        crate::cli::release_submit::RELEASE_BUILD_RUN_SCOPE
    ))
}

/// How many blobs a pass deleted, as the reap summary counts them.
fn count(deleted: &[String]) -> Result<i64, StorageError> {
    i64::try_from(deleted.len()).map_err(|error| {
        StorageError::Other(format!(
            "{} deleted blobs do not fit the reap summary's count: {error}",
            deleted.len()
        ))
    })
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
        return count(&[]);
    };
    let retained_manifest: Value = serde_json::from_str(&retained.content)?;
    crate::queue::submit::validate_stored_run_manifest(&retained_manifest, run_id)
        .map_err(|error| StorageError::Other(error.to_string()))?;
    if !has_complete_retained_outcomes(&retained_manifest) {
        return Err(StorageError::Other(format!(
            "run {run_id} cannot be swept without complete retained terminal outcomes"
        )));
    }
    let mut deleted: Vec<String> = Vec::new();
    for job_id in job_ids {
        // A prepared transition is a lifecycle companion, not an independent
        // source of truth. Resolve it through the canonical transition
        // protocol against the retained terminal destination before removing
        // any stale projection; never patch a job document here.
        store.recover_job_transition(job_id).await?;
        for prefix in TERMINAL_PREFIXES
            .iter()
            .copied()
            .chain(crate::queue::control::WATCHED_PREFIXES.iter().copied())
        {
            let blob = format!("{prefix}/{job_id}.json");
            if store.backend().exists(&blob).await? {
                store.delete_job(prefix, job_id).await?;
                deleted.push(blob);
            }
        }
        store.repair_priority_markers(job_id, None).await?;
        if !release_build_submission(run_id) {
            delete_status_dir(store, job_id).await?;
        }
    }
    // A lost swap means another writer changed the manifest; the next read
    // sees what it wrote. The loop ends on what it reads: the marker already
    // recorded, the manifest gone, the marker written, or any other error.
    loop {
        let Some(versioned) = store.read_text_versioned(&path).await? else {
            return count(&deleted);
        };
        let mut manifest: Value = serde_json::from_str(&versioned.content)?;
        if marked(&manifest, CLEANUP_COMPLETED_AT, run_id)? {
            return count(&deleted);
        }
        let object = manifest.as_object_mut().ok_or_else(|| {
            StorageError::Other(format!("run manifest {run_id} is not a JSON object"))
        })?;
        object.insert(
            CLEANUP_COMPLETED_AT.into(),
            Value::from(stamp()),
        );
        match store
            .compare_and_swap_text(
                &path,
                &versioned.version,
                &serde_json::to_string_pretty(&manifest)?,
            )
            .await
        {
            Ok(_) => return count(&deleted),
            Err(StorageError::StorageConflict(_)) => continue,
            Err(StorageError::NotFound(_)) => return count(&deleted),
            Err(error) => return Err(error),
        }
    }
}
