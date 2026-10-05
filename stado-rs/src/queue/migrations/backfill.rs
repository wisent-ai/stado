//! The sweep itself: the marker names already in the index, and the pass over
//! `queue/` that writes whatever is missing.

use std::collections::HashSet;

use futures::StreamExt;

use crate::models::Job;
use crate::queue::storage::JobStorage;
use crate::queue::{listing, StorageError};

use super::budgets::bulk_workers;
use super::prune::prune_stale_markers;
use super::sentinel::write_sentinel;

/// Every marker name that already exists, replacing Python
/// `_existing_marker_job_ids`.
///
/// Names, not job_ids, because the name is what the backfill can compute: it
/// holds the Job, so [`super::listing::marker_path`] tells it exactly which
/// object should exist. Recovering a job_id from a name is not possible
/// anyway (see [`super::listing::is_marker`]) — the old parse took the
/// segment after the last `-` and so never matched a real id, which made
/// every pass rewrite every marker it had just confirmed.
///
/// Comparing names also catches the case comparing ids could not: a marker
/// sitting under a superseded key is not the marker this job needs, so the
/// current one still gets written.
///
/// [`super::listing::marker_path`]: crate::queue::listing::marker_path
/// [`super::listing::is_marker`]: crate::queue::listing::is_marker
async fn existing_marker_names(store: &JobStorage) -> Result<HashSet<String>, StorageError> {
    let mut out = HashSet::new();
    for path in store.list_paths(listing::MARKER_PREFIX, 0).await? {
        if listing::is_marker(&path) {
            out.insert(path);
        }
    }
    Ok(out)
}

/// Scan every queued job and write any missing marker, then prune markers
/// that name no queued job. Returns the same coverage answer [`has_swept`]
/// reads, so a caller that already ran a sweep needs no second read.
///
/// NOT cheap: two whole-prefix name listings plus every queued job document.
/// Callers that only need to know whether the index is covered MUST ask
/// [`has_swept`]; this is the repair, and it belongs on a tick, not on a
/// scheduler poll. A tick processes what is due, so no batch size is chosen
/// here: every call covers the whole prefix, and a marker lost after any
/// sweep is rewritten by the next one.
///
/// This is the repair that keeps an unindexed job reachable, and it is the
/// ONLY one: the widening from priority>0 to every queued job extends this
/// pass rather than adding a second mechanism beside it.
///
/// [`has_swept`]: super::has_swept
pub async fn backfill_priority_markers(store: &JobStorage) -> Result<bool, StorageError> {
    // Marker names FIRST, queue names second, and the order is load-bearing:
    // see `prune_stale_markers` for the race it closes.
    let have = existing_marker_names(store).await?;
    let paths: Vec<String> = store
        .list_paths("queue/", 0)
        .await?
        .into_iter()
        .filter(|p| p.ends_with(".json"))
        .collect();
    let queued_ids: HashSet<String> = paths
        .iter()
        .filter_map(|path| {
            path.rsplit('/')
                .next()
                .and_then(|name| name.strip_suffix(".json"))
                .map(str::to_string)
        })
        .collect();
    let bodies: Vec<Option<String>> = futures::stream::iter(&paths)
        .map(|path| store.download_text(path))
        .buffered(bulk_workers())
        .collect::<Vec<Result<Option<String>, StorageError>>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()?;
    for body in bodies.into_iter().flatten() {
        if body.is_empty() {
            continue;
        }
        let job = Job::from_json(&body)?;
        // Queued-only, deliberately: a marker must never be resurrected for a
        // job that has already left the queue, or the walk would resolve it
        // against a `queue/` blob that no longer exists forever. The priority
        // floor that used to sit beside this check is gone — the index covers
        // every queued job now, and `priority_key` orders priority 0 as
        // correctly as any other value.
        if job.state != crate::models::job_state::QUEUED {
            continue;
        }
        let current = listing::marker_path(&job);
        if !have.contains(&current) {
            store.write_priority_marker(&job).await?;
        }
        // And drop this job's SUPERSEDED entries. A queued job keeps exactly
        // one index position; the others are names left behind by an earlier
        // `created_at` or priority, and they are the bulk of the bloat that
        // stalled claiming — five queued jobs held about 1,325 markers each.
        // `prune_stale_markers` cannot see them, because they name a job that
        // IS queued; only the job's own current key distinguishes them, and
        // that key is derivable only here, where the body is in hand.
        let suffix = format!("-{}.json", job.job_id);
        let superseded: Vec<&String> = have
            .iter()
            .filter(|path| **path != current && path.ends_with(&suffix))
            .collect();
        for path in superseded {
            store.delete_blob(path).await?;
        }
    }
    prune_stale_markers(store, &have, &queued_ids).await?;
    write_sentinel(store, "", true).await?;
    Ok(true)
}
