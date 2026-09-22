//! One listing: which runs it is about, and each platform joined to its job.

use futures::StreamExt;
use serde_json::{Map, Value};

use crate::cli::CmdError;
use crate::queue::runs;
use crate::queue::storage::JobStorage;

use super::jobs::{build_seconds, candidate_prefixes, compiling_count, job_state_and_cost, previous_compile_total};
use super::{load_run_value, RUN_STATE_LEAF, RUN_STATE_PREFIX};

/// One platform leg joined to its queue job: which run it belongs to, which
/// platform it is, and — when the queue still holds the job — the lifecycle
/// prefix it sits under with the seconds it has cost.
type PlatformJoin = (usize, String, Option<(String, Option<i64>)>);

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
        let field = |key: &str| run[key].as_str().unwrap_or_default();
        self.product
            .is_none_or(|selected| field("product") == selected)
            && self
                .run
                .is_none_or(|prefix| field("run_id").starts_with(prefix))
            && self
                .version
                .is_none_or(|selected| field("version") == selected)
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
    let store = JobStorage::new()
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let mut ordered: Vec<String> = {
        let mut blobs = store
            .list_blobs_with_meta(RUN_STATE_PREFIX)
            .await
            .map_err(|error| CmdError::click(error.to_string()))?
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
    let mut runs = Vec::new();
    let mut examined = usize::default();
    for path in &ordered {
        if runs.len() >= limit || examined >= VERSION_SCAN_WINDOW {
            break;
        }
        examined += true as usize;
        let Some(run) = load_run_value(&store, path).await? else {
            continue;
        };
        if !filter.admits(&run) {
            continue;
        }
        runs.push(run);
    }
    // Older runs stay unread unless a live build needs its denominator; the
    // ones already consumed above cannot be that denominator.
    ordered.drain(..examined.min(ordered.len()));
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
                (
                    index,
                    platform,
                    job_state_and_cost(store, &job_id, prefixes).await,
                )
            }
        },
    ))
    .buffered(8)
    .collect()
    .await;
    for (index, platform, found) in answers {
        let Some((state, seconds)) = found else {
            continue;
        };
        let Some(record) = runs[index]["platforms"].get_mut(&platform) else {
            continue;
        };
        record["job_state"] = Value::String(state);
        if let Some(seconds) = seconds {
            record["build_seconds"] = Value::from(seconds);
        }
    }
    for run in &mut runs {
        // Where the run stands, decided once here: failed, published or
        // still going. The desktop console used to re-decide it by matching
        // the state word, which is the same list in a second language.
        let state = run["state"]
            .as_str()
            .and_then(crate::release_pipeline::ReleaseRunState::named);
        if let Some(state) = &state {
            run["phase"] = Value::String(state.phase().to_owned());
        }
        let live = state.is_some_and(|state| !state.finished() && !state.published());
        if !live {
            continue;
        }
        let product_name = run["product"].as_str().unwrap_or("").to_owned();
        let Some(platforms) = run["platforms"].as_object_mut() else {
            continue;
        };
        for (platform_name, record) in platforms.iter_mut() {
            let Some(job_id) = record["job_id"].as_str().map(str::to_owned) else {
                continue;
            };
            // The build's own progress, from the log the agent streams while
            // the job runs: crates compiled so far, measured against the same
            // count from this platform's previous run. cargo publishes no
            // total, so the previous run IS the honest denominator, and the
            // figure is labelled an estimate everywhere it is shown.
            if record["job_state"].as_str() == Some("running") {
                if let Some(compiled) = compiling_count(&store, &job_id).await {
                    let mut progress = Map::new();
                    progress.insert("compiled".into(), Value::from(compiled));
                    if let Some(total) =
                        previous_compile_total(&store, &older, &product_name, platform_name).await
                    {
                        progress.insert("of_previous_run".into(), Value::from(total));
                        if let Some(ratio) = (compiled * 100).checked_div(total) {
                            progress.insert("percent".into(), Value::from(ratio.min(99)));
                        }
                    }
                    record["compile_progress"] = Value::Object(progress);
                }
            }
        }
    }
    Ok(runs)
}
