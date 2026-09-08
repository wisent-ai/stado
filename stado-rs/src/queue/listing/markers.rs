//! Index entry writes and removals: the exact delete every hot path takes,
//! and the scanning repair path for the keys that cannot be derived.

use crate::models::Job;
use crate::queue::storage::JobStorage;
use crate::queue::{json_str, StorageError};

use super::keys::{is_marker, marker_path};
use super::MARKER_PREFIX;

/// Index entry for a queued job.
pub async fn write_marker(store: &JobStorage, job: &Job) -> Result<(), StorageError> {
    // Python `json.dumps({"job_id": ..., "priority": int(...)})` with
    // default separators.
    let body = format!(
        "{{\"job_id\": {}, \"priority\": {}}}",
        json_str(&job.job_id),
        job.priority
    );
    store.upload_text(&marker_path(job), &body).await
}

/// Drop the marker this job names.
///
/// Exact, so it costs one delete. Correct because the name is a function of
/// `priority` and `created_at`, and a queued job's marker is rewritten
/// whenever its priority changes.
pub async fn delete_marker_for(store: &JobStorage, job: &Job) -> Result<(), StorageError> {
    store.delete_blob(&marker_path(job)).await
}

/// Repair path: drop every marker naming `job_id`, whatever key it was
/// written under, by walking the index — except `keep`, when the caller has
/// already written the marker the job needs.
///
/// This is the only remaining reason to traverse, and it exists for the two
/// cases where the name is genuinely not computable: a marker orphaned from a
/// job that no longer exists, and a marker left under a superseded key after
/// a priority change (the pre-change key cannot be derived from the
/// post-change job). Everything on a hot path uses [`delete_marker_for`]
/// instead.
///
/// `keep` exists because the safe order for a re-key is write-then-clean, and
/// this scan matches on job_id: without an exception it would delete the very
/// marker the caller just wrote and leave the job unindexed, which is the
/// failure this ordering is meant to avoid.
///
/// Matched on the `-<job_id>.json` suffix, which is exact here because job
/// ids are fixed-shape (`job-` + hex) and so no id can be a dash-delimited
/// suffix of another.
pub async fn delete_markers_scanning(
    store: &JobStorage,
    job_id: &str,
    keep: Option<&str>,
) -> Result<(), StorageError> {
    let suffix = format!("-{job_id}.json");
    for path in store.list_paths(MARKER_PREFIX, 0).await? {
        if is_marker(&path) && path.ends_with(&suffix) && Some(path.as_str()) != keep {
            store.delete_blob(&path).await?;
        }
    }
    Ok(())
}
