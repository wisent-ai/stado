//! Reports read back from the run objects a submission maintains: what `stado
//! release status` prints and what the operator console serves.

use futures::StreamExt;
use serde_json::{Map, Value};

use crate::cli::CmdError;
use crate::queue::runs;
use crate::queue::storage::JobStorage;

/// One run object as raw JSON, or nothing when it is absent or unreadable.
async fn load_run_value(store: &JobStorage, path: &str) -> Result<Option<Value>, CmdError> {
    let Some(text) = store
        .download_text(path)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?
    else {
        return Ok(None);
    };
    Ok(serde_json::from_str::<Value>(&text).ok())
}

/// Where `run_state_path` puts every run object, and its leaf.
const RUN_STATE_PREFIX: &str = "runs/release-pipeline/";
const RUN_STATE_LEAF: &str = "/run.json";
/// How many run objects a listing reads before it stops looking: a product
/// or version filter is answered from the body of each run, one request per
/// run, and the whole history is not a bounded question. A run named by id
/// is picked from the listing before any read, so this never cuts it off.
pub(crate) const VERSION_SCAN_WINDOW: usize = 120;

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

/// How long one platform's build actually took, in seconds.
///
/// The run object records `created_at` and `updated_at` and nothing else, so
/// until this existed no surface in the fleet could say what a release cost.
/// The duration is not copied into the run: the job record owns it, and a
/// second copy is a second answer. A job still running reports the time it
/// has been running so far.
fn build_seconds(job: &crate::models::Job) -> Option<i64> {
    let moment = |value: Option<&str>| {
        value
            .filter(|text| !text.is_empty())
            .and_then(|text| chrono::DateTime::parse_from_rfc3339(text).ok())
            .map(|stamp| stamp.with_timezone(&chrono::Utc))
    };
    let started = moment(job.started_at.as_deref())?;
    let ended = moment(job.completed_at.as_deref())
        .or_else(|| moment(job.failed_at.as_deref()))
        .unwrap_or_else(chrono::Utc::now);
    Some((ended - started).num_seconds().max(0))
}

/// The lifecycle prefixes a platform in this state can be found under, so a
/// terminal run costs two reads per platform instead of six.
fn candidate_prefixes(platform_state: Option<&str>) -> &'static [&'static str] {
    match platform_state {
        Some("published" | "qualified") => &[runs::COMPLETED, runs::UPLOADED],
        Some("failed") => &[runs::FAILED, runs::CANCELLED],
        _ => &[
            runs::RUNNING,
            runs::QUEUE,
            runs::COMPLETED,
            runs::UPLOADED,
            runs::FAILED,
            runs::CANCELLED,
        ],
    }
}

/// The queue state one job sits in and what it has cost so far.
async fn job_state_and_cost(
    store: &JobStorage,
    job_id: &str,
    prefixes: &[&str],
) -> Option<(String, Option<i64>)> {
    for state in prefixes {
        match store.read_job(state, job_id).await {
            Ok(Some(job)) => return Some(((*state).to_string(), build_seconds(&job))),
            Ok(None) => continue,
            Err(_) => return None,
        }
    }
    None
}

/// Distinct crates the job's streamed log says were compiled so far.
async fn compiling_count(store: &JobStorage, job_id: &str) -> Option<u64> {
    let bytes = store
        .read_bytes(&format!("status/{job_id}/output/command_output.log"))
        .await
        .ok()
        .flatten()?;
    let text = String::from_utf8_lossy(&bytes);
    Some(
        text.lines()
            .filter(|line| line.trim_start().starts_with("Compiling "))
            .count() as u64,
    )
}

/// The compile count of the newest older run of the same product and
/// platform whose job finished — the denominator for the estimate.
///
/// `older` is the run objects newest-first that [`recent_runs`] did not need,
/// as paths: the answer is nearly always the first or second of them, so they
/// are downloaded one at a time and the walk stops at the first usable count.
async fn previous_compile_total(
    store: &JobStorage,
    older: &[String],
    product: &str,
    platform: &str,
) -> Option<u64> {
    for path in older {
        let Ok(Some(run)) = load_run_value(store, path).await else {
            continue;
        };
        if run["product"].as_str() != Some(product) {
            continue;
        }
        let record = &run["platforms"][platform];
        let Some(job_id) = record["job_id"].as_str() else {
            continue;
        };
        if let Some(count) = compiling_count(store, job_id).await {
            if count > u64::default() {
                return Some(count);
            }
        }
    }
    None
}
