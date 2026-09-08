//! The oldest-first pass over the lifecycle prefix: one metadata listing,
//! ordered by write time, downloaded until the window closes.

use std::collections::HashSet;

use futures::StreamExt;

use crate::models::Job;
use crate::queue::storage::JobStorage;
use crate::queue::StorageError;

use super::scan::JobScan;

/// The oldest-first pass over the prefix itself.
///
/// The prefix is listed once, with metadata, and ordered by write time before
/// anything is downloaded. Ordering after a cap is what starved a late-sorting
/// job; re-listing to widen a budget is what made a poll cost the whole prefix
/// repeatedly. One listing, one order, early exit.
pub(super) async fn collect_oldest_first(
    store: &JobStorage,
    prefix: &str,
    scan: &JobScan<'_>,
    out: &mut Vec<Job>,
    seen: &mut HashSet<String>,
    scanned: &mut usize,
) -> Result<(), StorageError> {
    if scan.window_full(out.len()) || scan.budget_spent(*scanned) {
        return Ok(());
    }
    let mut ordered: Vec<(chrono::DateTime<chrono::Utc>, String)> = Vec::new();
    for blob in store.list_blobs_with_meta(&format!("{prefix}/")).await? {
        let name = blob.name.rsplit('/').next().unwrap_or("");
        let job_id = name.strip_suffix(".json").unwrap_or(name);
        if !blob.name.ends_with(".json") || seen.contains(job_id) {
            continue;
        }
        // A blob that predates the metadata stamp carries no gpu_mem_gb and
        // is downloaded rather than assumed unfit. A corrupt integer raises,
        // so misbehaving metadata is reported instead of silently filtering.
        if let Some(mem_str) = blob.metadata.get("gpu_mem_gb") {
            let mem: i64 = mem_str.parse().map_err(|_| {
                StorageError::Other(format!(
                    "corrupt gpu_mem_gb metadata on {}: {mem_str:?}",
                    blob.name
                ))
            })?;
            if mem > scan.max_gpu_mem_gb {
                continue;
            }
        }
        ordered.push((
            blob.updated.unwrap_or_else(chrono::Utc::now),
            blob.name.clone(),
        ));
    }
    ordered.sort();
    let paths: Vec<String> = ordered.into_iter().map(|(_, name)| name).collect();
    for paths in paths.chunks(32) {
        let texts: Vec<Option<String>> = futures::stream::iter(paths)
            .map(|path| store.download_text(path))
            .buffered(32)
            .collect::<Vec<Result<Option<String>, StorageError>>>()
            .await
            .into_iter()
            .collect::<Result<Vec<_>, _>>()?;
        for data in texts.into_iter().flatten() {
            let job = Job::from_json(&data)?;
            *scanned += 1;
            if scan.accepts(&job) && seen.insert(job.job_id.clone()) {
                out.push(job);
                if scan.window_full(out.len()) {
                    return Ok(());
                }
            }
            if scan.budget_spent(*scanned) {
                return Ok(());
            }
        }
    }
    Ok(())
}
