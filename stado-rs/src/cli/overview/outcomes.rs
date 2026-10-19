//! The queue's job outcomes for `overview --json`: per-model counts, every
//! failed and completed job newest first, and the measured wall time of the
//! completed ones. Stado Desktop's Queue and Posture screens read this section
//! of the same document the CLI prints; the reader decides how many rows it
//! shows, so no bound is chosen here.
//!
//! The model and task of a job are read out of its command the way the
//! makespan history reads them; a command that names neither is listed with
//! null for each, never under an invented label. A stamp that does not parse
//! is listed under `throughput.unreadable_stamps` with the job, the field and
//! the parser's error: read as absent, its job would drop out of the measured
//! wall time and the newest-first order without a word.

use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, Utc};
use serde_json::{json, Value};

use crate::models::Job;
use crate::queue::runs;
use crate::queue::{JobStorage, StorageError};
use crate::scheduler::makespan::history::extract_model_task;

/// The states counted per model, under the names `jobs` uses.
const COUNTED: &[&str] = &[runs::QUEUE, runs::RUNNING, runs::COMPLETED, runs::FAILED];

/// The stamps read for the outcomes, keeping every one that did not parse.
#[derive(Default)]
struct Stamps {
    unreadable: Vec<Value>,
}

impl Stamps {
    /// `field` of `job` as a time; `None` when the job carries none, and also
    /// when it does not parse, which is then recorded with its error.
    fn read(&mut self, job: &Job, field: &str, value: Option<&str>) -> Option<DateTime<Utc>> {
        let text = value?;
        match DateTime::parse_from_rfc3339(text) {
            Ok(parsed) => Some(parsed.with_timezone(&Utc)),
            Err(error) => {
                self.unreadable.push(json!({
                    "job_id": job.job_id,
                    "field": field,
                    "value": text,
                    "error": error.to_string(),
                }));
                None
            }
        }
    }
}

fn stated(text: String) -> Value {
    if text.is_empty() {
        Value::Null
    } else {
        json!(text)
    }
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

    let mut stamps = Stamps::default();
    let mut failed: Vec<(Option<DateTime<Utc>>, &Job)> = jobs(runs::FAILED)
        .iter()
        .map(|job| (stamps.read(job, "failed_at", job.failed_at.as_deref()), job))
        .collect();
    failed.sort_by_key(|(at, _)| std::cmp::Reverse(*at));
    let failed: Vec<Value> = failed
        .into_iter()
        .map(|(_, job)| {
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

    // How long each finished job ran, when both of its stamps read: its
    // start, its seconds and the job.
    type Finished<'a> = (Option<DateTime<Utc>>, Option<i64>, &'a Job);
    let mut completed: Vec<Finished<'_>> = jobs(runs::COMPLETED)
        .iter()
        .map(|job| {
            let started = stamps.read(job, "started_at", job.started_at.as_deref());
            let ended = stamps.read(job, "completed_at", job.completed_at.as_deref());
            let wall = match (started, ended) {
                (Some(started), Some(ended)) => Some((ended - started).num_seconds()),
                (_, _) => None,
            };
            (ended, wall, job)
        })
        .collect();
    completed.sort_by_key(|(at, _, _)| std::cmp::Reverse(*at));
    let walls: Vec<i64> = completed.iter().filter_map(|(_, wall, _)| *wall).collect();
    let completed: Vec<Value> = completed
        .into_iter()
        .map(|(_, wall, job)| {
            let (model, task) = extract_model_task(&job.command);
            json!({
                "job_id": job.job_id,
                "model": stated(model),
                "task": stated(task),
                "wall_seconds": wall,
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
            "unreadable_stamps": stamps.unreadable,
        },
    }))
}
