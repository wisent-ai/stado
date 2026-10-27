//! What ran on one host, in the order it happened: every job of the queue
//! that this host's worker claimed, from its job record.
//!
//! The beacon (`stado host health`, `stado host link`) says what a host is
//! now; this says what it did. Each job the queue keeps under `running/` or a
//! terminal prefix whose worker allocation names one of the host's registry
//! hostnames, or the registry name itself, is one entry with the times its
//! record states. A job whose record names no worker allocation ran before
//! allocations were recorded or was never claimed, so it is not counted for
//! any host; the answer says how many such records it read. A time that
//! cannot be read is said beside the entry and never replaced by another.

use chrono::{DateTime, FixedOffset};
use serde_json::{json, Value};

use crate::models::Job;
use crate::queue::runs::{RUNNING, TERMINAL_PREFIXES};
use crate::queue::{JobStorage, StorageError};
use crate::targets::ComputeTarget;

/// Whether `allocated` is one of `target`'s names, without regard to case.
fn names_target(target: &ComputeTarget, allocated: &str) -> bool {
    let allocated = allocated.trim();
    target.name.eq_ignore_ascii_case(allocated)
        || target
            .hostnames
            .iter()
            .any(|hostname| hostname.eq_ignore_ascii_case(allocated))
}

/// One time field of a record, parsed, or why it cannot be.
fn time(field: &str, value: Option<&str>) -> Result<Option<DateTime<FixedOffset>>, String> {
    match value {
        None => Ok(None),
        Some(text) => DateTime::parse_from_rfc3339(text)
            .map(Some)
            .map_err(|error| format!("its {field} {text:?} is not an RFC 3339 time: {error}")),
    }
}

/// The time an entry is ordered and filtered by: its end when it ended, its
/// start while it runs, its creation when it never started.
fn entry(job: &Job, lifecycle: &str) -> (Option<DateTime<FixedOffset>>, Value) {
    let mut problems = Vec::new();
    let mut read = |field: &str, value: Option<&str>| match time(field, value) {
        Ok(parsed) => parsed,
        Err(problem) => {
            problems.push(problem);
            None
        }
    };
    let created = read(
        "created_at",
        Some(job.created_at.as_str()).filter(|text| !text.is_empty()),
    );
    let started = read("started_at", job.started_at.as_deref());
    let completed = read("completed_at", job.completed_at.as_deref());
    let failed = read("failed_at", job.failed_at.as_deref());
    let ended = completed.or(failed);
    let at = ended.or(started).or(created);
    let seconds = match (started, ended) {
        (Some(started), Some(ended)) => Some((ended - started).num_seconds()),
        _ => None,
    };
    (
        at,
        json!({
            "at": at.map(|at| at.to_rfc3339()),
            "job": job.job_id,
            "lifecycle": lifecycle,
            "state": job.state,
            "command": job.command,
            "kind": job.worker_allocation.as_ref().map(|allocation| allocation.kind.clone()),
            "created_at": job.created_at,
            "started_at": job.started_at,
            "completed_at": job.completed_at,
            "failed_at": job.failed_at,
            "seconds": seconds,
            "error": job.error,
            "unreadable": problems,
        }),
    )
}

/// Every job `target` ran, oldest first, at or after `since` when given.
pub async fn read(
    store: &JobStorage,
    target: &ComputeTarget,
    since: Option<DateTime<FixedOffset>>,
) -> Result<Value, StorageError> {
    let mut entries: Vec<(Option<DateTime<FixedOffset>>, Value)> = Vec::new();
    let mut unallocated = serde_json::Map::new();
    for lifecycle in std::iter::once(RUNNING).chain(TERMINAL_PREFIXES) {
        let mut without = Vec::new();
        for job in store.list_jobs(lifecycle).await? {
            let Some(allocation) = job.worker_allocation.as_ref() else {
                without.push(job.job_id.clone());
                continue;
            };
            if !names_target(target, &allocation.host) {
                continue;
            }
            let (at, value) = entry(&job, lifecycle);
            let before = match (since, at) {
                (Some(since), Some(at)) => at < since,
                _ => false,
            };
            if !before {
                entries.push((at, value));
            }
        }
        unallocated.insert(lifecycle.to_string(), Value::from(without.len()));
    }
    // Entries whose time could not be read sort last, in the order read.
    entries.sort_by(|left, right| match (left.0, right.0) {
        (Some(left), Some(right)) => left.cmp(&right),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => std::cmp::Ordering::Equal,
    });
    let seconds: i64 = entries
        .iter()
        .filter_map(|(_, value)| value.get("seconds").and_then(Value::as_i64))
        .sum();
    Ok(json!({
        "host": target.name,
        "hostnames": target.hostnames,
        "since": since.map(|since| since.to_rfc3339()),
        "jobs": entries.into_iter().map(|(_, value)| value).collect::<Vec<_>>(),
        "seconds": seconds,
        "records_without_allocation": unallocated,
    }))
}
