//! What the cleaner table adds up to, and the outcome it selects.

use crate::providers::local::disk_cleanup::janitor::state::report::build::utc_now;
use crate::providers::local::disk_cleanup::janitor::state::report::CleanupReport;
use crate::providers::local::disk_cleanup::rule::VolumeReading;

/// Every cleaner's deletions this pass.
fn deleted_items(report: &CleanupReport) -> i64 {
    [
        &report.hf,
        &report.weles,
        &report.builds,
        &report.clones,
        &report.workdirs,
        &report.job_outputs,
        &report.backup_twins,
        &report.release_store,
        &report.local_snapshots,
        &report.object_evidence,
        &report.agent_logs,
    ]
    .iter()
    .map(|cleaner| cleaner.deleted_items)
    .sum()
}

/// The outcome of a pass that ran its cleaners, judged on the volume reading
/// taken after them.
///
/// - `report_only`: a preview; nothing was removed.
/// - `reclaimed_below_threshold`: the volume is under the rule's threshold.
/// - `blocked_running_jobs`: still full, and the HuggingFace cache was left
///   alone because jobs were running.
/// - `partial_error`: still full, and a cleaner failed.
/// - `no_eligible_items`: still full, and nothing the fleet owns was left to
///   take — the rest of the volume is the user's.
/// - `still_full`: still full after deleting what was eligible.
pub(crate) fn select_outcome(enforcing: bool, report: &mut CleanupReport, after: VolumeReading) {
    report.free_bytes_after = Some(after.free_bytes);
    report.used_percent_after = Some(after.used_percent());
    report.outcome = if !enforcing {
        "report_only"
    } else if !after.full() {
        "reclaimed_below_threshold"
    } else if report.active_job_count > 0 {
        "blocked_running_jobs"
    } else if !report.errors.is_empty() {
        "partial_error"
    } else if deleted_items(report) == 0 {
        "no_eligible_items"
    } else {
        "still_full"
    }
    .to_string();
    if report.errors.is_empty() {
        report.last_success_at = Some(utc_now());
    }
}
