//! The queue's job outcomes for `overview --json`: per-model counts, every
//! failed and completed job newest first, and the measured wall time of the
//! completed ones. Stado Desktop's Queue and Posture screens read this section
//! of the same document the CLI prints; the reader decides how many rows it
//! shows, so no bound is chosen here.
//!
//! The model and task of a job are read out of its command the way the
//! makespan history reads them; a command that names neither is listed with
//! null for each, never under an invented label.

use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, Utc};
use serde_json::{json, Value};

use crate::models::Job;
use crate::queue::runs;
use crate::queue::{JobStorage, StorageError};
use crate::scheduler::makespan::history::extract_model_task;

/// The states counted per model, under the names `jobs` uses.
const COUNTED: &[&str] = &[runs::QUEUE, runs::RUNNING, runs::COMPLETED, runs::FAILED];

fn stamp(value: Option<&str>) -> Option<DateTime<Utc>> {
    value
        .and_then(|text| DateTime::parse_from_rfc3339(text).ok())
        .map(|parsed| parsed.with_timezone(&Utc))
}

fn stated(text: String) -> Value {
    if text.is_empty() {
        Value::Null
    } else {
        json!(text)
    }
}

/// How long a finished job ran, when both of its stamps read.
fn wall_seconds(job: &Job) -> Option<i64> {
    let started = stamp(job.started_at.as_deref())?;
    let ended = stamp(job.completed_at.as_deref())?;
    Some((ended - started).num_seconds())
}

/// The jobs `list_all_jobs` read under one lifecycle prefix; it answers every
/// prefix, an empty one as an empty list.
fn in_state<'a>(all: &'a BTreeMap<String, Vec<Job>>, state: &str) -> &'a [Job] {
    match all.get(state) {
        Some(jobs) => jobs.as_slice(),
        None => &[],
    }
}

pub(super) async fn job_outcomes(store: &JobStorage) -> Result<Value, StorageError> {
    let all = store.list_all_jobs().await?;
    let jobs = |state: &str| in_state(&all, state);

    // Per model, the count in each counted state. A command naming no model
    // is counted under null rather than a made-up model name.
    let labelled: Vec<(String, &str)> = COUNTED
        .iter()
        .flat_map(|state| {
            jobs(state)
                .iter()
                .map(move |job| (extract_model_task(&job.command).0, *state))
        })
        .collect();
    let models: BTreeSet<&String> = labelled.iter().map(|(model, _)| model).collect();
    let by_model: Vec<Value> = models
        .into_iter()
        .map(|model| {
            let count = |state: &str| {
                labelled
                    .iter()
                    .filter(|(named, counted)| named == model && *counted == state)
                    .count()
            };
            json!({
                "model": stated(model.clone()),
                "queue": count(runs::QUEUE),
                "running": count(runs::RUNNING),
                "completed": count(runs::COMPLETED),
                "failed": count(runs::FAILED),
            })
        })
        .collect();

    let mut failed: Vec<&Job> = jobs(runs::FAILED).iter().collect();
    failed.sort_by_key(|job| std::cmp::Reverse(stamp(job.failed_at.as_deref())));
    let failed: Vec<Value> = failed
        .into_iter()
        .map(|job| {
            let (model, task) = extract_model_task(&job.command);
            json!({
                "job_id": job.job_id,
                "model": stated(model),
                "task": stated(task),
                "error": job.error,
                "failed_at": job.failed_at,
            })
        })
        .collect();

    let mut completed: Vec<&Job> = jobs(runs::COMPLETED).iter().collect();
    completed.sort_by_key(|job| std::cmp::Reverse(stamp(job.completed_at.as_deref())));
    let walls: Vec<i64> = completed
        .iter()
        .filter_map(|job| wall_seconds(job))
        .collect();
    let completed: Vec<Value> = completed
        .into_iter()
        .map(|job| {
            let (model, task) = extract_model_task(&job.command);
            json!({
                "job_id": job.job_id,
                "model": stated(model),
                "task": stated(task),
                "wall_seconds": wall_seconds(job),
                "completed_at": job.completed_at,
            })
        })
        .collect();

    // The average wall time over the completed jobs whose stamps read, and
    // how many that is; none read is said as null, not as zero seconds.
    let average =
        (!walls.is_empty()).then(|| walls.iter().sum::<i64>() as f64 / walls.len() as f64);
    Ok(json!({
        "by_model": by_model,
        "failed": failed,
        "completed": completed,
        "throughput": {
            "average_wall_seconds_per_completed_job": average,
            "samples": walls.len(),
        },
    }))
}
