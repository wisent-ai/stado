//! One listing: which runs it is about, and each platform joined to its job.

use futures::StreamExt;
use serde_json::{Map, Value};

use crate::cli::CmdError;
use crate::queue::storage::JobStorage;

use super::jobs::{
    candidate_prefixes, compiling_count, job_state_and_cost, platform_required,
    previous_compile_total, receipt_reading, JobReading,
};
use super::{load_run_value, RUN_STATE_LEAF, RUN_STATE_PREFIX};

/// One platform leg joined to its queue job: which run it belongs to, which
/// platform it is, and the queue's answer — the lifecycle prefix the job sits
/// under with its cost and error, `None` when it is under none of the
/// prefixes read, or why a prefix could not be read.
type PlatformJoin = (usize, String, Result<Option<JobReading>, String>);

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
    let mut examined: Vec<String> = Vec::new();
    for path in &ordered {
        if runs.len() >= limit {
            break;
        }
        examined.push(path.clone());
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
                // a finished release build whose queue record the retained
                // run sweep already retired still has its receipt.
                let found = match job_state_and_cost(store, &job_id, prefixes).await {
                    Ok(None) => match job_state_and_cost(store, &job_id, prefixes).await {
                        Ok(None) => receipt_reading(store, &job_id).await,
                        found => found,
                    },
                    found => found,
                };
                (index, platform, found)
            }
        },
    ))
    .buffered(8)
    .collect()
    .await;
    for (index, platform, found) in answers {
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
                    let job_id = record["job_id"].as_str().unwrap_or_default().to_owned();
                    record["state"] = Value::String("failed".into());
                    record["failure"] = Value::String(format!(
                        "build job {job_id} is in no queue state (queue, running, completed, uploaded, failed, cancelled) on two walks of the lifecycle; the job was lost and this leg cannot finish — submit the release again"
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
                let job_id = record["job_id"].as_str().unwrap_or_default().to_owned();
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
        // Whether a failed leg fails the release is submit's rule, not this
        // listing's: only a platform the build manifest marks required does.
        // Every failed leg is weighed — one the run already recorded (a build
        // refresh can adopt a failed leg while the run stays waiting) as well
        // as one whose job was just seen ending — so state and phase agree
        // for the CLI line (which prints `state`), the JSON and the Desktop
        // console (which reads `phase`). The stored word is kept as
        // `recorded_state` for whoever resumes the run.
        let unfinished = state
            .as_ref()
            .is_some_and(|state| !state.finished() && !state.published());
        if unfinished && required_leg_failed(&store, run).await {
            let failed = crate::release_pipeline::ReleaseRunState::Failed;
            run["recorded_state"] = run["state"].clone();
            run["state"] = serde_json::to_value(&failed).expect("a run state serializes");
            run["phase"] = Value::String(failed.phase().to_owned());
            continue;
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

/// Whether any failed leg of `run` is one the build manifest marks required.
/// Each failed leg is labelled `required` true or false, or carries
/// `required_unknown` with the reason the manifest could not be read; the
/// first required failure becomes the run's `failure` when it has none.
async fn required_leg_failed(store: &JobStorage, run: &mut Value) -> bool {
    let Some(platforms) = run["platforms"].as_object() else {
        return false;
    };
    let failed: Vec<String> = platforms
        .iter()
        .filter(|(_, record)| record["state"].as_str() == Some("failed"))
        .map(|(name, _)| name.clone())
        .collect();
    let mut any = false;
    for platform in failed {
        match platform_required(store, run, &platform).await {
            Ok(required) => {
                run["platforms"][&platform]["required"] = Value::Bool(required);
                if required {
                    any = true;
                    if run["failure"].is_null() {
                        let text = match run["platforms"][&platform]["failure"].as_str() {
                            Some(text) => format!("{platform}: {text}"),
                            None => format!("{platform}: failed with no recorded reason"),
                        };
                        run["failure"] = Value::String(text);
                    }
                }
            }
            Err(why) => run["platforms"][&platform]["required_unknown"] = Value::String(why),
        }
    }
    any
}
