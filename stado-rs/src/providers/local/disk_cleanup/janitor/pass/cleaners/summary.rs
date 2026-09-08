//! What the cleaner table adds up to, and the outcome it selects.

use crate::providers::local::disk_cleanup::janitor::pass::cleaners::CLEANER_ORDER;
use crate::providers::local::disk_cleanup::janitor::state::report::build::utc_now;
use crate::providers::local::disk_cleanup::janitor::state::report::{CleanerReport, CleanupReport};
use crate::providers::local::disk_cleanup::janitor::GIB;
use crate::providers::local::disk_cleanup::{
    backup_twins, chromium_clones, queue_workdirs, release_store,
};
use crate::targets::DiskCleanupPolicy;

/// Which declared cleaners never got a turn, and which the policy names that
/// this binary does not implement. Moved out of `run_with_lock` unchanged.
pub(crate) fn summarize_scan(policy: &DiskCleanupPolicy, report: &mut CleanupReport) {
    let total_scanned = report.hf.scanned_items
        + report.weles.scanned_items
        + report.builds.scanned_items
        + report.clones.scanned_items
        + report.workdirs.scanned_items
        + report.backup_twins.scanned_items
        + report.release_store.scanned_items;
    if total_scanned >= policy.max_scan_items {
        report.caps.scan = true;
    }
    // Which declared cleaners never got a turn. Every counter needed for this
    // was already in hand here and nothing said it: a cleaner whose share ran
    // out publishes the same three zeros as one that looked and found nothing,
    // and `cap_reached` names the budget rather than the cleaner it stopped.
    //
    // Keyed on the two skips a budget produces — `scan_cap` and
    // `scan_deadline` — and never on a zero count alone: a cleaner whose root
    // does not exist on this host also scans nothing, reports `root_absent`,
    // and is not waiting for a turn. Calling that one unscanned would be this
    // field committing the error it exists to report. The order is the run
    // order, so the answer reads as "the pass ended before these".
    let budget_stopped = |cleaner: &CleanerReport| {
        cleaner.scanned_items == 0
            && (cleaner.skipped.contains_key("scan_cap")
                || cleaner.skipped.contains_key("scan_deadline"))
    };
    report.unscanned_cleaners = [
        ("huggingface_cache", &report.hf),
        ("weles_recordings", &report.weles),
        ("build_caches", &report.builds),
        (chromium_clones::CLEANER, &report.clones),
        (queue_workdirs::CLEANER, &report.workdirs),
        (backup_twins::CLEANER, &report.backup_twins),
        (release_store::CLEANER, &report.release_store),
    ]
    .into_iter()
    .filter(|(name, cleaner)| policy.cleaners.contains_key(*name) && budget_stopped(cleaner))
    .map(|(name, _)| name.to_string())
    .collect();
    report.unknown_cleaners = policy
        .cleaners
        .keys()
        .filter(|name| !CLEANER_ORDER.contains(&name.as_str()))
        .cloned()
        .collect();
}

/// The outcome this pass reports, and the success stamp that goes with it.
/// Moved out of `run_with_lock` unchanged.
pub(crate) fn select_outcome(policy: &DiskCleanupPolicy, report: &mut CleanupReport, after: i64) {
    report.free_bytes_after = Some(after);
    // Deliberately NOT `report.hf.deleted_items` alone, as the Python had
    // it: neither build_caches nor chromium_clones has a Python original to
    // stay faithful to, and a pass that removed 200 GB of tagged build trees
    // or 130 stale browser clones while the HF cache held nothing evictable
    // must not report `no_eligible_items`.
    let deleted = report.hf.deleted_items
        + report.builds.deleted_items
        + report.clones.deleted_items
        + report.backup_twins.deleted_items;
    // An incomplete scan is named before any complete-pass verdict. In
    // particular, running jobs may block one cleaner while a later cleaner
    // makes progress and exhausts the pass budget; that is still unfinished
    // work which must continue to the target on the next admitted pass.
    if report.caps.any() && after < policy.target_free_gb * GIB {
        report.outcome = "cap_reached".to_string();
    } else if policy.mode != "enforce" {
        report.outcome = "report_only".to_string();
    } else if after >= policy.target_free_gb * GIB {
        report.outcome = "reclaimed_target".to_string();
    } else if report.active_job_count > 0 && policy.cleaners.contains_key("huggingface_cache") {
        report.outcome = "blocked_running_jobs".to_string();
    } else if !report.errors.is_empty() {
        report.outcome = "partial_error".to_string();
    } else if deleted == 0 {
        report.outcome = "no_eligible_items".to_string();
    } else {
        report.outcome = "reclaimed_progress".to_string();
    }
    if report.errors.is_empty() {
        report.last_success_at = Some(utc_now());
    }
}
