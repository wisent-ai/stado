//! Reads over a lifecycle prefix itself: the bulk job fetch, and the
//! id-only walk that answers from the listing alone.

use futures::StreamExt;

use crate::models::Job;
use crate::queue::storage::{is_transition_sentinel_state, JobStorage};
use crate::queue::StorageError;

/// Parallel-fetch job JSONs under `{prefix}/`.
///
/// `oldest_first > 0` caps the answer to that many jobs picked by creation
/// time — required for queue/ where 14k+ blobs would otherwise force the
/// scheduler to download every JSON before slicing per_tick_cap and blow the
/// 60s function timeout. Fetches fan out 10 ways.
///
/// The window is oldest-first, and the ordered path listing is taken EXACTLY
/// ONCE: the scheduler's FIFO fairness is "the N oldest queued blobs", and a
/// re-listing loop that widens a budget pays for the whole prefix again on
/// every round. Transitional sentinels are skipped without being counted, so
/// the walk simply continues down the already-ordered list until the window
/// holds N live jobs or the prefix is exhausted.
pub async fn list_jobs(
    store: &JobStorage,
    prefix: &str,
    oldest_first: usize,
) -> Result<Vec<Job>, StorageError> {
    let ordered = if oldest_first > 0 { usize::MAX } else { 0 };
    // The prefix is a DIRECTORY, so it is matched at the delimiter and not as
    // a string. The store answers `starts_with`, and `queue/` is a string
    // prefix of `queue_priority/`: on 2026-09-03 this listing asked for the 13
    // objects under `queue/` and got 9,032, because every priority marker
    // matched too. Downloading those took thousands of round trips through the
    // loopback resolver, one of them failed, and the whole call returned an
    // error -- which surfaced as `the queue store is unreadable` and blocked
    // the 0.13.49 release train at `release-capacity` for an hour.
    //
    // Two prefixes in this store are string prefixes of others (`queue` of
    // `queue_priority`, `failed` of `failed_again`), so this is a live
    // ambiguity rather than a hypothetical one.
    let directory = format!("{prefix}/");
    let paths: Vec<String> = store
        .list_paths(&directory, ordered)
        .await?
        .into_iter()
        .filter(|path| path.starts_with(&directory) && path.ends_with(".json"))
        .collect();
    let mut jobs = Vec::new();
    for paths in paths.chunks(100) {
        let texts: Vec<Option<String>> = futures::stream::iter(paths)
            .map(|path| store.download_text(path))
            .buffered(10)
            .collect::<Vec<Result<Option<String>, StorageError>>>()
            .await
            .into_iter()
            .collect::<Result<Vec<_>, _>>()?;
        for data in texts.into_iter().flatten() {
            let job = Job::from_json(&data)?;
            if is_transition_sentinel_state(&job.state) {
                continue;
            }
            jobs.push(job);
            if oldest_first > 0 && jobs.len() >= oldest_first {
                return Ok(jobs);
            }
        }
    }
    Ok(jobs)
}

/// Every job id under `{prefix}/`, from the LISTING ALONE.
///
/// A job blob is written at exactly `{prefix}/{job_id}.json` (`storage.rs`
/// spells that key at every transition), so the id a caller wants is already
/// in the name and downloading the document to read it back out is a round
/// trip per job for a field the listing handed over for free.
///
/// That is not a micro-optimisation. The janitor's workdir keep-list used
/// [`list_jobs`] for this, and on 2026-09-03 charless-mac-mini spent
/// `duration_ms:
/// 818021` — 13.6 minutes — on a cleanup pass whose own verdict
/// was `healthy_noop` on a host with 19.8 GB free, because the pass downloaded
/// every object the `queue` listing returned before it had decided whether any
/// cleaner would run. The keep-list needs a set of ids, and this returns one
/// for one listing per 1000 names instead of one GET per name.
///
/// DELIBERATELY a superset of [`list_jobs`]: a job whose blob currently holds
/// a transition sentinel keeps its id here, where `list_jobs` drops it. For a
/// keep-list that is the only safe direction — an id present too often keeps a
/// directory alive one pass longer, an id missing authorizes deleting the tree
/// a mid-transition job is writing into — and a caller that wants job DOCUMENTS
/// still has [`list_jobs`].
pub async fn list_job_ids(store: &JobStorage, prefix: &str) -> Result<Vec<String>, StorageError> {
    // Matched at the delimiter for the reason [`list_jobs`] states: `queue/`
    // is a string prefix of `queue_priority/`, and a listing that answers
    // `starts_with` hands over both.
    let directory = format!("{prefix}/");
    Ok(store
        .list_paths(&directory, 0)
        .await?
        .into_iter()
        .filter_map(|path| {
            let name = path.strip_prefix(&directory)?.strip_suffix(".json")?;
            // Job blobs sit flat under the prefix; anything nested is not one.
            (!name.is_empty() && !name.contains('/')).then(|| name.to_string())
        })
        .collect())
}
