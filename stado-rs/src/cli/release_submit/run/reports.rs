//! Reports read back from the run objects a submission maintains: what `stado
//! release status` prints and what the operator console serves.

use serde_json::{Map, Value};

use crate::cli::CmdError;
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
    let store = JobStorage::new()
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let mut ordered: Vec<String> = {
        let mut blobs = store
            .list_blobs_with_meta("runs/release-pipeline/")
            .await
            .map_err(|error| CmdError::click(error.to_string()))?
            .into_iter()
            .filter(|blob| blob.name.ends_with("/run.json"))
            .collect::<Vec<_>>();
        blobs.sort_by_key(|blob| std::cmp::Reverse(blob.updated));
        blobs.into_iter().map(|blob| blob.name).collect()
    };
    let mut runs = Vec::new();
    let mut examined = usize::default();
    for path in &ordered {
        if runs.len() >= limit {
            break;
        }
        examined += true as usize;
        let Some(run) = load_run_value(&store, path).await? else {
            continue;
        };
        if product.is_some_and(|selected| run["product"].as_str() != Some(selected)) {
            continue;
        }
        runs.push(run);
    }
    // Older runs stay unread unless a live build needs its denominator; the
    // ones already consumed above cannot be that denominator.
    ordered.drain(..examined.min(ordered.len()));
    let older = ordered;
    // An in-flight run says only "publishing", which reads as a promise. The
    // run object already names each platform's queue job, and the queue knows
    // exactly where that job stands, so the two are joined here: every
    // platform of a live run carries the job's current queue state. Terminal
    // runs are left alone — their platform states are already the answer.
    for run in &mut runs {
        let live = matches!(
            run["state"].as_str(),
            Some("submitting" | "waiting" | "publishing" | "delivering")
        );
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
            for state in [
                "running",
                "queue",
                "completed",
                "uploaded",
                "failed",
                "cancelled",
            ] {
                match store.read_job(state, &job_id).await {
                    Ok(Some(_)) => {
                        record["job_state"] = Value::String(state.to_string());
                        break;
                    }
                    Ok(None) => continue,
                    Err(_) => break,
                }
            }
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
