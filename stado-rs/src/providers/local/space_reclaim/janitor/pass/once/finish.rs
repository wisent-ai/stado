//! Finishing a pass: persisting its state and carrying forward what a pass
//! that reached no cleaner did not itself establish.

use std::path::Path;
use std::time::Instant;

use serde_json::Value;

use crate::providers::local::disk_cleanup::janitor::policy::roots::free_bytes;
use crate::providers::local::disk_cleanup::janitor::state::read_state;
use crate::providers::local::disk_cleanup::janitor::state::report::canonical::canonical_json;
use crate::providers::local::disk_cleanup::janitor::state::report::CleanupReport;
use crate::providers::local::disk_cleanup::janitor::state::write::write_state;

/// Python `_finish`.
pub(crate) fn finish(
    mut report: CleanupReport,
    started: Instant,
    home: Option<&Path>,
    state_dir: Option<&Path>,
    attempted_at: f64,
    log_fn: &mut dyn FnMut(&str),
) -> Value {
    if let Some(home) = home {
        if let Ok(free) = free_bytes(home) {
            report.free_bytes_after = Some(free);
        }
    }
    report.duration_ms = (started.elapsed().as_secs_f64() * 1000.0).max(0.0) as i64;
    if let Some(state_dir) = state_dir {
        let value = report.to_value();
        if let Err(exc) = write_state(state_dir, &value, attempted_at) {
            report.add_error("state_write", &exc);
            if !matches!(
                report.outcome.as_str(),
                "lock_busy" | "lock_busy_workloads" | "volume_unreadable"
            ) {
                report.outcome = "partial_error".to_string();
            }
        }
    }
    let value = report.to_value();
    let line = canonical_json(&value);
    // Python swallows logging failures.
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| log_fn(&line)));
    value
}

/// A pass prevented by the lock keeps the previous pass's success stamp and
/// target name: it observed the lock, not the host.
pub(crate) fn preserve_previous_report(state_dir: &Path, report: &mut CleanupReport) {
    let Ok(previous) = read_state(state_dir) else {
        return;
    };
    let Some(previous) = previous.get("report").and_then(Value::as_object) else {
        return;
    };
    report.last_success_at = previous
        .get("last_success_at")
        .and_then(Value::as_str)
        .map(str::to_string);
    report.target_name = previous
        .get("target_name")
        .and_then(Value::as_str)
        .map(str::to_string);
}
