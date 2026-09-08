//! Supervision verdicts for a recorded run: where its job actually is, and
//! when silence has lasted long enough to be a failure.

use chrono::{DateTime, Utc};

use crate::queue::runs::TERMINAL_PREFIXES;
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

/// The terminal prefix `job_id` has landed in, or `None` while it is still
/// queued or running — or has been swept out of the queue entirely. The
/// caller distinguishes "still in flight" from "vanished" with
/// [`stuck_reason`]: absence alone is not a verdict.
pub(super) async fn terminal_prefix(
    store: &JobStorage,
    job_id: &str,
) -> Result<Option<&'static str>, String> {
    for prefix in TERMINAL_PREFIXES {
        let found = store
            .read_job(prefix, job_id)
            .await
            .map_err(|exc| format!("reading {prefix}/{job_id}: {exc}"))?;
        if found.is_some() {
            return Ok(Some(prefix));
        }
    }
    Ok(None)
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
/// Build jobs carry no `runs/` manifest, so the by-run reaper never sweeps
/// their records: absence here is disappearance, not housekeeping.
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
