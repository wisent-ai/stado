//! The next pass a periodic writer promises, and whether a pass was turned
//! away by the run lock.
//!
//! A writer with a declared period states in the state file when its next
//! pass will have written that file. `host gates` judges the janitor by that
//! statement: past it, nobody is making passes. Nothing here is a window
//! somebody picked; the promise is the writer's period plus what passes on
//! this host have been measured to take and to land late by.

use serde_json::{Map, Value};

/// The state-file key holding each periodic writer's promise.
pub(crate) const PROMISES: &str = "promises";

fn recorded(promise: Option<&Value>, key: &str) -> Option<f64> {
    promise
        .and_then(|promise| promise.get(key))
        .and_then(Value::as_f64)
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
    let mut promises = previous
        .get(PROMISES)
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let writer = report.get("writer").and_then(Value::as_str);
    let every = report.get("every_seconds").and_then(Value::as_u64);
    let (Some(writer), Some(every)) = (writer, every) else {
        return promises;
    };
    let pid = report.get("writer_pid").and_then(Value::as_u64);
    let duration = report
        .get("duration_ms")
        .and_then(Value::as_f64)
        .unwrap_or(0.0)
        .max(0.0)
        / 1000.0;
    let standing = promises.get(writer);
    let longest_pass = recorded(standing, "longest_pass_seconds")
        .unwrap_or(0.0)
        .max(duration);
    let same_process = pid.is_some()
        && standing
            .and_then(|promise| promise.get("pid"))
            .and_then(Value::as_u64)
            == pid;
    let lateness = recorded(standing, "finished_at")
        .filter(|_| same_process)
        .map(|finished_at| (attempted_at - finished_at - every as f64).max(0.0));
    let longest_lateness = recorded(standing, "longest_lateness_seconds")
        .unwrap_or(0.0)
        .max(lateness.unwrap_or(0.0));
    let finished_at = attempted_at + duration;
    let next_pass_by = finished_at + every as f64 + longest_pass + longest_lateness;
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
    let outcome = report.get("outcome").and_then(Value::as_str);
    outcome == Some("lock_busy")
        || outcome == Some("lock_busy_workloads")
        || (outcome == Some("lock_busy_unattributed")
            && report
                .get("active_job_count")
                .and_then(Value::as_i64)
                .is_some_and(|count| count > 0))
}
