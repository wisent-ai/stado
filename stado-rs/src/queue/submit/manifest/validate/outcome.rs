//! A run entry's retained terminal outcome, proved against the planned job
//! it ends: its fields, its prefix and job state, and every time it records
//! inside the run's lifetime.

use chrono::{DateTime, Utc};
use serde_json::Value;

use crate::models::Job;
use crate::queue::submit::SubmitError;

const OUTCOME_FIELDS: &[&str] = &["prefix", "recorded_at", "job"];

/// An RFC 3339 instant the manifest records, refused naming it and why.
fn recorded_instant(value: &str, what: &str) -> Result<DateTime<Utc>, SubmitError> {
    DateTime::parse_from_rfc3339(value)
        .map(|instant| instant.with_timezone(&Utc))
        .map_err(|error| {
            SubmitError::Validation(format!("{what} {value:?} is not RFC 3339: {error}"))
        })
}

/// The outcome an ended `entry` at `index` of run `run_id` retains for
/// `job`, refused naming what does not hold.
pub(super) fn terminal_outcome(
    entry: &Value,
    outcome_job: Option<&Job>,
    job: &Job,
    run_id: &str,
    index: usize,
    manifest_created_at: DateTime<Utc>,
) -> Result<(), SubmitError> {
    let outcome = entry
        .get("outcome")
        .and_then(Value::as_object)
        .ok_or_else(|| SubmitError::Validation("terminal entry has no outcome".into()))?;
    if outcome.len() != OUTCOME_FIELDS.len()
        || !OUTCOME_FIELDS
            .iter()
            .all(|field| outcome.contains_key(*field))
    {
        return Err(SubmitError::Validation(
            "terminal entry outcome has invalid fields".into(),
        ));
    }
    let Some(prefix) = outcome.get("prefix").and_then(Value::as_str) else {
        return Err(SubmitError::Validation(
            "terminal outcome prefix is not text".into(),
        ));
    };
    let terminal_job = outcome_job
        .ok_or_else(|| SubmitError::Validation("terminal entry has no retained job".into()))?;
    if !crate::queue::runs::TERMINAL_PREFIXES.contains(&prefix) || terminal_job.state != prefix {
        return Err(SubmitError::Validation(
            "terminal entry prefix and retained job state disagree".into(),
        ));
    }
    let Some(recorded_raw) = outcome.get("recorded_at").and_then(Value::as_str) else {
        return Err(SubmitError::Validation(
            "terminal outcome recorded_at is not text".into(),
        ));
    };
    let recorded_at = recorded_instant(recorded_raw, "terminal outcome recorded_at")?;
    // No clock-skew allowance is invented: a future outcome is refused.
    if recorded_at < manifest_created_at || recorded_at > Utc::now() {
        return Err(SubmitError::Validation(
            "terminal outcome recorded_at is outside the run lifetime".into(),
        ));
    }
    // The prefix is one of TERMINAL_PREFIXES (checked above): a failed
    // job ends at its failure, every other terminal job at completion.
    let terminal_at = if prefix == crate::queue::runs::FAILED {
        terminal_job
            .completed_at
            .is_none()
            .then_some(terminal_job.failed_at.as_deref())
            .flatten()
    } else {
        terminal_job
            .failed_at
            .is_none()
            .then_some(terminal_job.completed_at.as_deref())
            .flatten()
    }
    .ok_or_else(|| {
        SubmitError::Validation(
            "terminal retained job has contradictory terminal timestamps".into(),
        )
    })?;
    let terminal_at = recorded_instant(terminal_at, "terminal retained job timestamp")?;
    if terminal_at < manifest_created_at || terminal_at > recorded_at {
        return Err(SubmitError::Validation(
            "terminal retained job timestamp is outside the outcome lifetime".into(),
        ));
    }
    if !crate::queue::runs::terminal_job_matches_entry(terminal_job, job, run_id, index) {
        return Err(SubmitError::Validation(format!(
            "stable job key {} belongs to different submission content",
            job.job_id
        )));
    }
    Ok(())
}
