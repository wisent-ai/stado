//! Finishing a pass: persisting its state and carrying forward what a pass
//! that reached no cleaner did not itself establish.

use std::path::Path;
use std::time::Instant;

use serde_json::Value;

use crate::providers::local::space_reclaim::janitor::policy::roots::free_bytes;
use crate::providers::local::space_reclaim::janitor::state::read_state;
use crate::providers::local::space_reclaim::janitor::state::report::build::elapsed_ms;
use crate::providers::local::space_reclaim::janitor::state::report::canonical::canonical_json;
use crate::providers::local::space_reclaim::janitor::state::report::CleanupReport;
use crate::providers::local::space_reclaim::janitor::state::write::write_state;

/// Python `_finish`.
pub(crate) fn finish(
    mut report: CleanupReport,
    started: Instant,
    home: Option<&Path>,
    state_dir: Option<&Path>,
    attempted_at: f64,
    log_fn: &mut dyn FnMut(&str),
) -> Value {
    // The closing reading is part of the report: a volume that could not be
    // read after the cleaners is said beside the pass, not left as unmeasured
    // in silence.
    if let Some(home) = home {
        match free_bytes(home) {
            Ok(free) => report.free_bytes_after = Some(free),
            Err(exc) => report.add_error("free_bytes_after", &exc),
        }
    }
    report.duration_ms = Some(elapsed_ms(started));
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
    // A log sink that panics must not end the pass that already ran; that it
    // did is said on standard error.
    if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| log_fn(&line))).is_err() {
        eprintln!("stado: the space-reclaim log sink panicked while writing this pass's report");
    }
    value
}

/// A pass prevented by the lock keeps the previous pass's success stamp and
/// target name: it observed the lock, not the host. A previous state that
/// cannot be read is said in the report, not taken for a host never cleaned.
pub(crate) fn preserve_previous_report(state_dir: &Path, report: &mut CleanupReport) {
    let previous = match read_state(state_dir) {
        Ok(previous) => previous,
        Err(exc) => {
            report.add_error("state_read", &exc);
            return;
        }
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
