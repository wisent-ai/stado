//! The listing-only keep set the workdir cleaner refuses to delete.

use std::collections::BTreeSet;
use std::time::Duration;

/// Every job id named by `queue` or `running`, without downloading job
/// documents. Transition sentinels stay on this conservative set.
async fn listed_live_job_ids(store: &crate::queue::JobStorage) -> Option<BTreeSet<String>> {
    let mut ids = BTreeSet::new();
    for state in ["queue", "running"] {
        ids.extend(store.list_job_ids(state).await.ok()?);
    }
    Some(ids)
}

/// Build the conservative listing-only keep set inside `budget`.
///
/// Public as the existing seam that proves no queue document is downloaded.
pub async fn live_job_ids_within(
    store: &crate::queue::JobStorage,
    budget: Duration,
) -> Option<Vec<String>> {
    tokio::time::timeout(budget, listed_live_job_ids(store))
        .await
        .ok()
        .flatten()
        .map(BTreeSet::into_iter)
        .map(Iterator::collect)
}

/// Refine the listing-only keep set for the bounded workdir population.
///
/// Only ids that exist on disk and whose names occur in both a live and a
/// terminal prefix cost authoritative document reads. The typed storage query
/// accepts only a retired transition with its matching terminal destination
/// and no live or in-flight source. Any failed list/read or timeout makes the
/// whole keep-list unavailable, so the workdir cleaner deletes nothing.
async fn live_job_ids_for_candidates_within(
    store: &crate::queue::JobStorage,
    candidates: &BTreeSet<String>,
    budget: Duration,
) -> Option<Vec<String>> {
    let read = async {
        let mut live = listed_live_job_ids(store).await?;
        let mut terminal_names = BTreeSet::new();
        for prefix in crate::queue::runs::TERMINAL_PREFIXES {
            terminal_names.extend(store.list_job_ids(prefix).await.ok()?);
        }
        for job_id in candidates {
            if !live.contains(job_id) || !terminal_names.contains(job_id) {
                continue;
            }
            match store.workdir_job_state(job_id).await.ok()? {
                crate::queue::storage::WorkdirJobState::Terminal => {
                    live.remove(job_id);
                }
                crate::queue::storage::WorkdirJobState::Live
                | crate::queue::storage::WorkdirJobState::Unknown => {}
            }
        }
        Some(live.into_iter().collect())
    };
    tokio::time::timeout(budget, read).await.ok().flatten()
}

/// Build the refined keep-list against this process's configured authoritative
/// primary. Construction and layout validation are part of the same budget as
/// every listing and versioned read.
pub(crate) async fn fetch_live_job_ids(
    candidates: &BTreeSet<String>,
    budget: Duration,
) -> Option<Vec<String>> {
    let read = async {
        let store = crate::queue::JobStorage::for_primary_reads().await.ok()?;
        live_job_ids_for_candidates_within(&store, candidates, budget).await
    };
    tokio::time::timeout(budget, read).await.ok().flatten()
}
