//! What a build cost, and how far it has got.
//!
//! The duration is not copied into the run object: the job record owns it, and
//! a second copy is a second answer. A job still running reports the time it
//! has been running so far, so a release in flight is readable rather than
//! blank.

use crate::cli::release_submit::run::source::build_path;
use crate::queue::runs;
use crate::queue::storage::JobStorage;
use crate::release_control;
use crate::release_pipeline::{self, ProductManifest};

use super::load_run_value;

/// How long one platform's build actually took, in seconds.
///
/// The run object records `created_at` and `updated_at` and nothing else, so
/// until this existed no surface in the fleet could say what a release cost.
/// The duration is not copied into the run: the job record owns it, and a
/// second copy is a second answer. A job still running reports the time it
/// has been running so far.
pub(super) fn build_seconds(job: &crate::models::Job) -> Option<i64> {
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
pub(super) fn candidate_prefixes(platform_state: Option<&str>) -> &'static [&'static str] {
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

/// What the queue says about one platform's job: the lifecycle prefix it sits
/// under, the seconds it has cost, and — when it ended failed or cancelled —
/// the error the job recorded.
pub(super) type JobReading = (String, Option<i64>, Option<String>);

/// The queue state one job sits in, what it has cost so far, and why it
/// failed when it did.
pub(super) async fn job_state_and_cost(
    store: &JobStorage,
    job_id: &str,
    prefixes: &[&str],
) -> Option<JobReading> {
    for state in prefixes {
        match store.read_job(state, job_id).await {
            Ok(Some(job)) => {
                let ended_badly = matches!(*state, runs::FAILED | runs::CANCELLED);
                let error = ended_badly.then(|| {
                    job.error
                        .clone()
                        .filter(|text| !text.trim().is_empty())
                        .unwrap_or_else(|| {
                            format!("the job recorded no error; stado job watch {job_id}")
                        })
                });
                return Some(((*state).to_string(), build_seconds(&job), error));
            }
            Ok(None) => continue,
            Err(_) => return None,
        }
    }
    None
}

/// Distinct crates the job's streamed log says were compiled so far.
pub(super) async fn compiling_count(store: &JobStorage, job_id: &str) -> Option<u64> {
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
pub(super) async fn previous_compile_total(
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
            if count > 0 {
                return Some(count);
            }
        }
    }
    None
}

/// Whether a run's platform is one the release cannot ship without, by the
/// manifest the run was built from — the same policy submit applies when a
/// leg fails (`run/submit.rs`: only a required platform fails the release).
/// `Err` says why the manifest could not be read, so the reader is told the
/// question is open instead of being handed a guess.
pub(super) async fn platform_required(
    store: &JobStorage,
    run: &serde_json::Value,
    platform: &str,
) -> Result<bool, String> {
    let product = run["product"].as_str().unwrap_or_default();
    let Some(build_id) = run["build_id"].as_str() else {
        return Err("the run names no build, so its manifest cannot be read".into());
    };
    let path = build_path(product, build_id, "manifest.json");
    let bytes = store
        .read_bytes(&path)
        .await
        .map_err(|error| format!("{path}: {error}"))?
        .ok_or_else(|| format!("build manifest is missing: {path}"))?;
    if run["manifest_sha256"].as_str() != Some(release_control::sha256_bytes(&bytes).as_str()) {
        return Err(format!("{path} does not match the run's manifest digest"));
    }
    let ProductManifest::Release(manifest) = release_pipeline::parse_product_manifest(&bytes)?
    else {
        return Err(format!("{path} disables releases"));
    };
    manifest
        .platforms
        .get(platform)
        .map(|recipe| recipe.required)
        .ok_or_else(|| format!("{path} declares no platform {platform}"))
}
