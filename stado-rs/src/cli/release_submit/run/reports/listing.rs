//! One listing: which runs it is about, and each platform joined to its job.

use futures::StreamExt;
use serde_json::Value;

use crate::cli::CmdError;
use crate::queue::storage::JobStorage;

use super::jobs::{candidate_prefixes, job_state_and_cost, settled_reading, JobReading};
use super::phase;
use super::{load_run_value, RUN_STATE_LEAF, RUN_STATE_PREFIX};

/// One platform leg joined to its queue job: which run it belongs to, which
/// platform it is, the job id it was looked up by, and the queue's answer —
/// the lifecycle prefix the job sits under with its cost and error, `None`
/// when it is under none of the prefixes read, or why a prefix could not be
/// read. The id travels with the answer, so a failure names the job that was
/// looked for instead of re-reading the record with an empty-text default.
type PlatformJoin = (usize, String, String, Result<Option<JobReading>, String>);

/// Which runs a listing is about: one product, one run by id prefix, one
/// version. Every field left `None` matches every run.
#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct RunFilter<'a> {
    pub product: Option<&'a str>,
    /// A run id or its first characters, as `release status` prints them.
    pub run: Option<&'a str>,
    pub version: Option<&'a str>,
}

impl RunFilter<'_> {
    fn admits(&self, run: &Value) -> bool {
        // A run without the field is not one the filter selects; read as ""
        // it matched an empty --run prefix as if it carried that id.
        let field = |key: &str| run[key].as_str();
        self.product
            .is_none_or(|selected| field("product") == Some(selected))
            && self.run.is_none_or(|prefix| {
                field("run_id").is_some_and(|run_id| run_id.starts_with(prefix))
            })
            && self
                .version
                .is_none_or(|selected| field("version") == Some(selected))
    }
}

/// The most recent pipeline runs, newest first, with their persisted
/// failures.
///
/// This is the read side of the run objects `submit` maintains: `stado
/// release status` prints it and the dashboard's operator console serves the
/// same text, so a failed run is visible from the CLI and the GUI without
/// hunting through hosts or job stores.
///
/// The listing already carries each run object's write time, so the order is
/// known before a single body is downloaded and the read stops as soon as
/// `limit` matching runs are in hand. Downloading every run.json ever written
/// to then sort and truncate is the same shape as the autonomy outcome tick:
/// a bounded question answered with the whole history, over a store that
/// serves one object per HTTP request — and the release console polls this.
pub(crate) async fn recent_runs(
    product: Option<&str>,
    limit: usize,
) -> Result<Vec<Value>, CmdError> {
    matching_runs(
        RunFilter {
            product,
            ..RunFilter::default()
        },
        limit,
    )
    .await
}
/// The newest `limit` runs the filter admits, newest first. A run named by
/// id is found however far back it is: the walk reads run objects until the
/// filter is satisfied or the listing ends, which is what `release status
/// --run` needs and what a bounded window cannot give.
pub(crate) async fn matching_runs(
    filter: RunFilter<'_>,
    limit: usize,
) -> Result<Vec<Value>, CmdError> {
    let store = JobStorage::new().await.map_err(CmdError::from)?;
    let mut ordered: Vec<String> = {
        let mut blobs = store
            .list_blobs_with_meta(RUN_STATE_PREFIX)
            .await
            .map_err(CmdError::from)?
            .into_iter()
            .filter(|blob| blob.name.ends_with(RUN_STATE_LEAF))
            // The run id is the path segment after the prefix, so a run
            // named by id is found from the listing alone; a version is
            // not in the path and costs one read per run examined.
            .filter(|blob| {
                filter.run.is_none_or(|prefix| {
                    blob.name[RUN_STATE_PREFIX.len().min(blob.name.len())..].starts_with(prefix)
                })
            })
            .collect::<Vec<_>>();
        blobs.sort_by_key(|blob| std::cmp::Reverse(blob.updated));
        blobs.into_iter().map(|blob| blob.name).collect()
    };
    // Each run object is one request to a store that serves one object per
    // request, and the product a run belongs to is not in its path, so a
    // product's runs are found by reading the newest objects. They are read
    // as the bulk budget allows, in the listing's order, and the walk stops
    // as soon as `limit` admitted runs are in hand; read one after another,
    // a product with one old run waited on every run object ever written.
    let mut runs = Vec::new();
    let mut examined: Vec<String> = Vec::new();
    {
        let mut reads = futures::stream::iter(ordered.iter().map(|path| {
            let store = &store;
            async move { (path, load_run_value(store, path).await) }
        }))
        .buffered(crate::queue::migrations::bulk_workers()?);
        while runs.len() < limit {
            let Some((path, loaded)) = reads.next().await else {
                break;
            };
            examined.push(path.clone());
            let Some(run) = loaded? else {
                continue;
            };
            if filter.admits(&run) {
                runs.push(run);
            }
        }
    }
    // Older runs stay unread unless a live build needs its denominator; the
    // ones already consumed above cannot be that denominator.
    ordered.retain(|path| !examined.contains(path));
    let older = ordered;
    // An in-flight run says only "publishing", which reads as a promise, and a
    // finished one says "reconciled" without ever saying what it cost. The run
    // object already names each platform's queue job, and the job record is
    // where `started_at` and `completed_at` live, so the two are joined here:
    // every platform carries the job's queue state and the seconds it spent.
    // The reads fan out, and a terminal platform is looked for only in the two
    // prefixes its own state allows, so the join stays cheap enough for a
    // command the release console polls.
    let mut requests = Vec::new();
    for (index, run) in runs.iter().enumerate() {
        let Some(platforms) = run["platforms"].as_object() else {
            continue;
        };
        for (platform_name, record) in platforms {
            let Some(job_id) = record["job_id"].as_str().filter(|id| !id.is_empty()) else {
                continue;
            };
            requests.push((
                index,
                platform_name.clone(),
                job_id.to_owned(),
                candidate_prefixes(record["state"].as_str()),
            ));
        }
    }
    let answers: Vec<PlatformJoin> = futures::stream::iter(requests.into_iter().map(
        |(index, platform, job_id, prefixes)| {
            let store = &store;
            async move {
                // A move fences its source before it writes the destination,
                // so a job in transition is briefly under no prefix. One more
                // walk tells that window from a job that is really gone, and
                // a build job whose queue record the run reaper already
                // retired is read from what it left: its receipt, the
                // outcome its run retained, or its transition record.
                let found = match job_state_and_cost(store, &job_id, prefixes).await {
                    Ok(None) => match job_state_and_cost(store, &job_id, prefixes).await {
                        Ok(None) => settled_reading(store, &job_id).await,
                        found => found,
                    },
                    found => found,
                };
                (index, platform, job_id, found)
            }
        },
    ))
    .buffered(crate::queue::migrations::bulk_workers()?)
    .collect()
    .await;
    for (index, platform, job_id, found) in answers {
        let (state, seconds, error) = match found {
            Ok(Some(reading)) => reading,
            // A leg still in flight was looked for under every lifecycle
            // prefix. A job in none of them was lost: nothing will ever move
            // it, so "waiting" would be read forever. The leg fails with that
            // finding, and submit's rule below decides whether the run does.
            Ok(None) => {
                let Some(record) = runs[index]["platforms"].get_mut(&platform) else {
                    continue;
                };
                let in_flight = !matches!(
                    record["state"].as_str(),
                    Some("published" | "qualified" | "failed")
                );
                if in_flight {
                    record["state"] = Value::String("failed".into());
                    record["failure"] = Value::String(format!(
                        "build job {job_id} is in no queue state (queue, running, completed, uploaded, failed, cancelled) on two walks of the lifecycle, left no receipt, no reaped run retains it and no transition record names how it ended; the job was lost and this leg cannot finish — submit the release again"
                    ));
                }
                continue;
            }
            Err(reason) => {
                if let Some(record) = runs[index]["platforms"].get_mut(&platform) {
                    record["job_read_error"] = Value::String(reason);
                }
                continue;
            }
        };
        let Some(record) = runs[index]["platforms"].get_mut(&platform) else {
            continue;
        };
        // The run object moves a platform to failed only when submit or
        // resume next looks at it, so a job that already ended failed would
        // read as in flight with no failure. The job record is the authority
        // on how the build ended; its error is shown on the leg. A leg the
        // run already records as failed keeps the failure text it has.
        if let Some(error) = error {
            if record["state"].as_str() != Some("failed") {
                record["state"] = Value::String("failed".into());
                record["failure"] =
                    Value::String(format!("build job {job_id} ended {state}: {error}"));
            }
        }
        let Some(record) = runs[index]["platforms"].get_mut(&platform) else {
            continue;
        };
        record["job_state"] = Value::String(state);
        if let Some(seconds) = seconds {
            record["build_seconds"] = Value::from(seconds);
        }
    }
    phase::settle(&store, &mut runs, &older).await?;
    Ok(runs)
}
