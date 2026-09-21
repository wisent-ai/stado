//! The listing-only keep set the workdir cleaner refuses to delete.
//!
//! Every read here used to collapse into `None`: a store that could not be
//! opened, a listing the primary refused, a document read that failed and a
//! budget that ran out all produced the same wordless answer, and the
//! cleaners turned it into `queue_store_unreadable` with nothing after it.
//! On charless-mac-mini on 2026-09-21 that one word stood in front of 34.9
//! GiB of finished jobs' outputs while the host sat 15.4 GiB below its disk
//! target and was refused as a release builder — and nothing in the report
//! said which of the four it was. Each failure now carries the sentence the
//! store gave, so the janitor's own report names the read that failed.

use std::collections::BTreeSet;

use crate::providers::local::disk_cleanup::janitor::state::error::JanitorError;

fn unreadable(operation: &str, detail: &str) -> JanitorError {
    JanitorError::os(&format!("the queue store could not {operation}: {detail}"))
}

/// Every job id named by `queue` or `running`, without downloading job
/// documents. Transition sentinels stay on this conservative set.
async fn listed_live_job_ids(
    store: &crate::queue::JobStorage,
) -> Result<BTreeSet<String>, JanitorError> {
    let mut ids = BTreeSet::new();
    for state in ["queue", "running"] {
        let listed = store
            .list_job_ids(state)
            .await
            .map_err(|error| unreadable(&format!("list the {state} prefix"), &error.to_string()))?;
        ids.extend(listed);
    }
    Ok(ids)
}

/// Build the conservative listing-only keep set.
///
/// Public as the existing seam that proves no queue document is downloaded.
/// A read that fails leaves the keep-list unavailable and the cleaner
/// deletes nothing; a read that is slow is still a read, and waiting for it
/// deletes nothing either.
pub async fn live_job_ids_within(store: &crate::queue::JobStorage) -> Option<Vec<String>> {
    listed_live_job_ids(store)
        .await
        .ok()
        .map(BTreeSet::into_iter)
        .map(Iterator::collect)
}

/// Refine the listing-only keep set for the bounded workdir population.
///
/// Only ids that exist on disk and whose names occur in both a live and a
/// terminal prefix cost authoritative document reads. The typed storage query
/// accepts only a retired transition with its matching terminal destination
/// and no live or in-flight source. Any failed list or read leaves the whole
/// keep-list unavailable, so the workdir cleaner deletes nothing — but it
/// leaves it unavailable with a reason.
async fn live_job_ids_for_candidates_within(
    store: &crate::queue::JobStorage,
    candidates: &BTreeSet<String>,
) -> Result<Vec<String>, JanitorError> {
    let mut live = listed_live_job_ids(store).await?;
    let mut terminal_names = BTreeSet::new();
    for prefix in crate::queue::runs::TERMINAL_PREFIXES {
        let listed = store.list_job_ids(prefix).await.map_err(|error| {
            unreadable(&format!("list the {prefix} prefix"), &error.to_string())
        })?;
        terminal_names.extend(listed);
    }
    for job_id in candidates {
        if !live.contains(job_id) || !terminal_names.contains(job_id) {
            continue;
        }
        let state = store.workdir_job_state(job_id).await.map_err(|error| {
            unreadable(
                &format!("read the state of job {job_id}"),
                &error.to_string(),
            )
        })?;
        match state {
            crate::queue::storage::WorkdirJobState::Terminal => {
                live.remove(job_id);
            }
            crate::queue::storage::WorkdirJobState::Live
            | crate::queue::storage::WorkdirJobState::Unknown => {}
        }
    }
    Ok(live.into_iter().collect())
}

/// The candidates the queue POSITIVELY lists as terminal: named under a
/// terminal prefix and under no live one. A candidate the store lists
/// nowhere stays out of the answer, which is the difference from the
/// workdir keep-list: a work tree of an unknown job is scratch, but a job's
/// durable output may be a record this host cannot see the owner of, and
/// the job-outputs cleaner deletes only what the queue has retired by name.
async fn terminal_job_ids_for_candidates_within(
    store: &crate::queue::JobStorage,
    candidates: &BTreeSet<String>,
) -> Result<BTreeSet<String>, JanitorError> {
    let live = listed_live_job_ids(store).await?;
    let mut terminal = BTreeSet::new();
    for prefix in crate::queue::runs::TERMINAL_PREFIXES {
        let listed = store.list_job_ids(prefix).await.map_err(|error| {
            unreadable(&format!("list the {prefix} prefix"), &error.to_string())
        })?;
        terminal.extend(listed);
    }
    Ok(candidates
        .iter()
        .filter(|job_id| terminal.contains(*job_id) && !live.contains(*job_id))
        .cloned()
        .collect())
}

/// The positively terminal subset of `candidates`, against this process's
/// configured authoritative primary. An error is the reason the caller
/// removes nothing, and it reaches the janitor's report.
pub(crate) async fn fetch_terminal_job_ids(
    candidates: &BTreeSet<String>,
) -> Result<BTreeSet<String>, JanitorError> {
    let store = crate::queue::JobStorage::for_primary_reads()
        .await
        .map_err(|error| unreadable("be opened for primary reads", &error.to_string()))?;
    terminal_job_ids_for_candidates_within(&store, candidates).await
}

/// Build the refined keep-list against this process's configured
/// authoritative primary.
pub(crate) async fn fetch_live_job_ids(
    candidates: &BTreeSet<String>,
) -> Result<Vec<String>, JanitorError> {
    let store = crate::queue::JobStorage::for_primary_reads()
        .await
        .map_err(|error| unreadable("be opened for primary reads", &error.to_string()))?;
    live_job_ids_for_candidates_within(&store, candidates).await
}
