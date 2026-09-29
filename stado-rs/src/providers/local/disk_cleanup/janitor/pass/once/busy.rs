//! A run lock held by running workloads: who they are, and the pass's request
//! for its turn.

use std::path::Path;

use serde_json::Value;

use crate::providers::local::disk_cleanup::janitor::pass::lock::holds;
use crate::providers::local::disk_cleanup::janitor::policy::resolve_canonical_policy;
use crate::providers::local::disk_cleanup::janitor::policy::roots::free_bytes;
use crate::providers::local::disk_cleanup::janitor::state::error::JanitorError;
use crate::providers::local::disk_cleanup::janitor::state::report::build::epoch_now;
use crate::providers::local::disk_cleanup::janitor::state::report::CleanupReport;
use crate::providers::local::disk_cleanup::janitor::GIB;

/// Name the running workloads that hold the lock in shared mode and count
/// them. While one live shared hold exists no exclusive hold can, so they are
/// the whole answer. Below the declared low watermark a pass that may delete
/// also asks for its turn, so no new workload takes a hold until a pass has
/// had the lock. Returns false when no live workload holds it.
pub(super) fn describe_workloads(
    state_dir: &Path,
    home: &Path,
    registry: &Result<Value, JanitorError>,
    may_request_turn: bool,
    report: &mut CleanupReport,
    log_fn: &mut dyn FnMut(&str),
) -> bool {
    let workloads = holds::live(state_dir);
    if workloads.is_empty() {
        return false;
    }
    let now = epoch_now();
    let named = workloads
        .iter()
        .map(|hold| {
            format!(
                "{} (pid {}, held {:.0}s)",
                hold.holder,
                hold.pid,
                (now - hold.acquired_at).max(0.0)
            )
        })
        .collect::<Vec<_>>()
        .join("; ");
    let mut detail = format!(
        "held in shared mode by {} running workload(s): {named}",
        workloads.len()
    );
    report.active_job_count = report.active_job_count.max(workloads.len() as i64);
    report.outcome = "lock_busy_workloads".to_string();
    if may_request_turn && below_low_watermark(home, registry, &report.hostname) {
        match holds::request_turn(state_dir, &named) {
            Ok(()) => detail.push_str(
                "; the host is below its low watermark, so new workloads wait until a pass has run",
            ),
            Err(error) => report.add_error("cleanup_turn", &error),
        }
    }
    log_fn(&format!("disk cleanup: lock {detail}"));
    report.add_error("lock_busy", &JanitorError::os(&detail));
    true
}

fn below_low_watermark(
    home: &Path,
    registry: &Result<Value, JanitorError>,
    hostname: &str,
) -> bool {
    let Some(low) = registry
        .as_ref()
        .ok()
        .and_then(|data| resolve_canonical_policy(data, hostname).ok())
        .map(|(_, policy, _, _)| policy.low_free_gb * GIB)
    else {
        return false;
    };
    free_bytes(home).is_ok_and(|free| free < low)
}
