//! The next pass a periodic writer promises, and whether a pass was turned
//! away by the run lock.
//!
//! A writer with a declared period states in the state file when its next
//! pass will have written that file. `host gates` judges the janitor by that
//! statement: past it, nobody is making passes. Nothing here is a window
//! somebody picked; the promise is the writer's period plus what passes on
//! this host have been measured to take and to land late by.

use std::time::Duration;

use serde_json::{Map, Value};

/// The state-file key holding each periodic writer's promise.
pub(crate) const PROMISES: &str = "promises";

fn recorded(promise: Option<&Value>, key: &str) -> Option<f64> {
    promise
        .and_then(|promise| promise.get(key))
        .and_then(Value::as_f64)
}

/// The longer of two measurements, either of which may not exist yet.
fn longest(recorded: Option<f64>, measured: Option<f64>) -> Option<f64> {
    [recorded, measured].into_iter().flatten().reduce(f64::max)
}

/// The promises carried forward from `previous`, with this pass's writer's
/// replaced when `report` came from a periodic writer. A single pass promises
/// nothing and leaves every standing promise as it was.
///
/// The promise is this pass's end plus the writer's period, the longest pass
/// on record for it, and the longest it has been measured to start late: the
/// time between one pass's end and the next pass's start beyond the period,
/// spent writing this file, printing, and in the agent's case sweeping
/// scratch and restoring reconcilers. Lateness is measured only between two
/// passes of one process, so a stopped service does not count as late; a
/// writer's first pass on a host has no lateness on record yet. Both
/// measurements belong to the host and are kept across restarts.
pub(crate) fn promises_after(
    previous: &Value,
    report: &Value,
    attempted_at: f64,
) -> Map<String, Value> {
    // No standing promises is the state of a host whose writer never passed.
    let mut promises = match previous.get(PROMISES).and_then(Value::as_object) {
        Some(standing) => standing.clone(),
        None => Map::new(),
    };
    let writer = report.get("writer").and_then(Value::as_str);
    let every = report.get("every_seconds").and_then(Value::as_u64);
    let (Some(writer), Some(every)) = (writer, every) else {
        return promises;
    };
    let every = Duration::from_secs(every).as_secs_f64();
    let pid = report.get("writer_pid").and_then(Value::as_u64);
    // A pass that reported no duration adds no measurement; it is not a pass
    // that took no time.
    let duration = report
        .get("duration_ms")
        .and_then(Value::as_u64)
        .map(|millis| Duration::from_millis(millis).as_secs_f64());
    let standing = promises.get(writer);
    let longest_pass = longest(recorded(standing, "longest_pass_seconds"), duration);
    let same_process = pid.is_some()
        && standing
            .and_then(|promise| promise.get("pid"))
            .and_then(Value::as_u64)
            == pid;
    // A pass that started on time or early was not late at all.
    let lateness = recorded(standing, "finished_at")
        .filter(|_| same_process)
        .map(|finished_at| attempted_at - finished_at - every)
        .filter(|late| late.is_sign_positive());
    let longest_lateness = longest(recorded(standing, "longest_lateness_seconds"), lateness);
    let finished_at = attempted_at + duration.into_iter().sum::<f64>();
    let next_pass_by = finished_at
        + every
        + [longest_pass, longest_lateness]
            .into_iter()
            .flatten()
            .sum::<f64>();
    promises.insert(
        writer.to_string(),
        serde_json::json!({
            "pid": pid,
            "every_seconds": every,
            "finished_at": finished_at,
            "longest_pass_seconds": longest_pass,
            "longest_lateness_seconds": longest_lateness,
            "next_pass_by": next_pass_by,
        }),
    );
    promises
}

/// Whether the pass `report` describes was turned away by a workload's hold
/// on the run lock rather than run.
///
/// A live workload takes only the kernel's shared hold and writes no holder
/// record, so its answer is `lock_busy_unattributed`; that is a prevention
/// only when the same pass also counted one of this host's own jobs live.
/// Without that evidence a legacy or foreign holder must not make a silent
/// janitor look healthy.
pub(crate) fn pass_was_prevented(report: &Value) -> bool {
    match report.get("outcome").and_then(Value::as_str) {
        Some("lock_busy" | "lock_busy_workloads") => true,
        Some("lock_busy_unattributed") => report
            .get("active_job_count")
            .and_then(Value::as_i64)
            .is_some_and(i64::is_positive),
        _ => false,
    }
}
