//! Supervision verdicts for a recorded run: where its job actually is, and
//! when silence has lasted long enough to be a failure.

use chrono::{DateTime, Utc};
use serde_json::Value;

use crate::queue::runs::{RUN_PREFIX, TERMINAL_PREFIXES};
use crate::queue::storage::JobStorage;
use crate::targets::BuildRun;

/// A build job still queued this long after submission counts as unclaimed:
/// either no live worker of its platform exists, or every one of them is
/// refusing work — both states the run record must say, not a `running`
/// that means nothing. Capacity publications go stale in 180s, so 600s is
/// three missed publications, never one slow tick.
const QUEUE_CLAIM_THRESHOLD_SECONDS: i64 = 600;

/// Wall-clock ceiling on one claimed build job, measured from its
/// `started_at` (its submission time when the job record carries none).
/// v1 is a fixed ceiling for every recipe: a build that legitimately takes
/// longer needs a recipe field, not a longer silence.
const BUILD_CEILING_SECONDS: i64 = 3600;

/// The terminal prefix this run's job has landed in, or `None` while it is
/// still queued or running — or has been swept out of the queue entirely. The
/// caller distinguishes "still in flight" from "vanished" with
/// [`stuck_reason`]: absence alone is not a verdict.
///
/// Two places hold that fact, and both are authoritative in turn. While the
/// job blob is live, the prefix it sits in is the answer. Once the by-run
/// reaper has retired the run, the blob is gone and the answer is the outcome
/// the reaper retained into the durable manifest — which it writes before it
/// deletes anything. Reading only the live prefixes is how a build that
/// really completed was recorded as a job that disappeared, in the very tick
/// that retired it.
pub(super) async fn terminal_prefix(
    store: &JobStorage,
    run: &BuildRun,
) -> Result<Option<&'static str>, String> {
    for prefix in TERMINAL_PREFIXES {
        let found = store
            .read_job(prefix, &run.job_id)
            .await
            .map_err(|exc| format!("reading {prefix}/{}: {exc}", run.job_id))?;
        if found.is_some() {
            return Ok(Some(prefix));
        }
    }
    retained_prefix(store, run).await
}

/// The prefix the by-run reaper retained for this job inside its durable
/// submission manifest, or `None` when the run declares no manifest, the
/// manifest is gone, or the entry that names this job carries no outcome yet.
///
/// The manifest is addressed by the id the run recorded at submission, so this
/// is one read, never a scan of every run the store holds.
async fn retained_prefix(
    store: &JobStorage,
    run: &BuildRun,
) -> Result<Option<&'static str>, String> {
    if run.run_id.is_empty() {
        return Ok(None);
    }
    let path = format!("{RUN_PREFIX}/{}.json", run.run_id);
    let Some(text) = store
        .download_text(&path)
        .await
        .map_err(|exc| format!("reading {path}: {exc}"))?
    else {
        return Ok(None);
    };
    let manifest: Value =
        serde_json::from_str(&text).map_err(|error| format!("{path} is not JSON: {error}"))?;
    let retained = manifest
        .get("entries")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .find(|entry| entry.get("job_id").and_then(Value::as_str) == Some(run.job_id.as_str()))
        .and_then(|entry| entry.get("outcome"))
        .and_then(|outcome| outcome.get("prefix"))
        .and_then(Value::as_str);
    Ok(retained.and_then(|prefix| {
        TERMINAL_PREFIXES
            .iter()
            .copied()
            .find(|known| *known == prefix)
    }))
}

/// The supervision verdict for a run whose job sits in no terminal prefix:
/// the one-sentence reason the run must be failed now, or `None` while it
/// is still inside its budgets.
///
/// Three budgets, three sentences, because they send the operator to three
/// different places: a job still queued past [`QUEUE_CLAIM_THRESHOLD_SECONDS`]
/// says no worker took the work; a claimed job past [`BUILD_CEILING_SECONDS`]
/// says the build — or the worker running it — is wedged; a job record gone
/// from every prefix says the record was lost with no outcome ever reported.
/// A build job does belong to a `runs/` manifest, so the by-run reaper does
/// sweep its record — which is why [`terminal_prefix`] reads the outcome that
/// reaper retained before this verdict is ever reached. Reaching it means no
/// live prefix and no retained outcome hold the job.
pub(super) async fn stuck_reason(
    store: &JobStorage,
    run: &BuildRun,
    log: &dyn Fn(&str),
) -> Option<String> {
    let recorded_at = DateTime::parse_from_rfc3339(&run.at)
        .ok()?
        .with_timezone(&Utc);
    let age = (Utc::now() - recorded_at).num_seconds().max(0);
    match store.read_job("queue", &run.job_id).await {
        Ok(Some(_)) => {
            return (age > QUEUE_CLAIM_THRESHOLD_SECONDS)
                .then(|| "no worker claimed the job within 10m".to_string());
        }
        Ok(None) => {}
        Err(exc) => {
            log(&format!(
                "build job {}: reading queue record: {exc}",
                run.job_id
            ));
            return None;
        }
    }
    match store.read_job("running", &run.job_id).await {
        Ok(Some(job)) => {
            let running_age = job
                .started_at
                .as_deref()
                .and_then(|text| DateTime::parse_from_rfc3339(text).ok())
                .map(|started| {
                    (Utc::now() - started.with_timezone(&Utc))
                        .num_seconds()
                        .max(0)
                })
                .unwrap_or(age);
            return (running_age > BUILD_CEILING_SECONDS)
                .then(|| "job exceeded the 60m build ceiling".to_string());
        }
        Ok(None) => {}
        Err(exc) => {
            log(&format!(
                "build job {}: reading running record: {exc}",
                run.job_id
            ));
            return None;
        }
    }
    Some("job record disappeared; the worker never reported".to_string())
}
