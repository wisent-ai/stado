//! Finishing a pass: persisting its state and preserving what a pass
//! that reached no cleaner must not overwrite.

use std::path::Path;
use std::time::Instant;

use serde_json::Value;

use crate::providers::local::disk_cleanup::build_caches;
use crate::providers::local::disk_cleanup::janitor::policy::roots::free_bytes;
use crate::providers::local::disk_cleanup::janitor::state::report::canonical::canonical_json;
use crate::providers::local::disk_cleanup::janitor::state::report::CleanupReport;
use crate::providers::local::disk_cleanup::janitor::state::write::write_state;
use crate::providers::local::disk_cleanup::janitor::state::{read_state, ControlUpdateAuthority};
use crate::providers::local::disk_cleanup::janitor::MAX_ERRORS;

/// Python `_finish`.
pub(crate) fn finish(
    mut report: CleanupReport,
    started: Instant,
    home: Option<&Path>,
    state_dir: Option<&Path>,
    attempted_at: f64,
    control_update: ControlUpdateAuthority,
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
        if let Err(exc) = write_state(
            state_dir,
            &value,
            report.builds_cursor.as_ref(),
            attempted_at,
            control_update,
        ) {
            report.add_error("state_write", &exc);
            if report.outcome != "lock_busy" && report.outcome != "invalid_or_unavailable_policy" {
                report.outcome = "partial_error".to_string();
            }
        }
    }
    report.errors.truncate(MAX_ERRORS);
    let value = report.to_value();
    let line = canonical_json(&value);
    // Python swallows logging failures.
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| log_fn(&line)));
    value
}

pub(crate) fn preserve_previous_report(state_dir: &Path, report: &mut CleanupReport) {
    let Ok(previous) = read_state(state_dir) else {
        return;
    };
    report.builds_cursor = build_caches::BuildCachesCursor::from_state(&previous);
    report.backup_cursor =
        crate::providers::local::disk_cleanup::backup_twins::cursor::BackupCursor::from_state(
            &previous,
        );
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
    report.policy_digest = previous
        .get("policy_digest")
        .and_then(Value::as_str)
        .map(str::to_string);
    report.policy_defaulted = previous
        .get("policy_defaulted")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    report.mode = previous
        .get("mode")
        .and_then(Value::as_str)
        .map(str::to_string);
    report.check_interval_seconds = previous
        .get("check_interval_seconds")
        .and_then(Value::as_i64);
    report.low_bytes = previous.get("low_bytes").and_then(Value::as_i64);
    report.target_bytes = previous.get("target_bytes").and_then(Value::as_i64);
    report.pressure_active = previous.get("pressure_active").and_then(Value::as_bool);
    report.builds_resume_from = previous
        .get("build_caches_resume_from")
        .and_then(Value::as_str)
        .map(str::to_string);
    report.unscanned_cleaners = previous
        .get("unscanned_cleaners")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
}
