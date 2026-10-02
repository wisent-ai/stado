//! The fixed cleaner order and one pass's run of every declared cleaner.
//!
//! The rebuildable-cache cleaners run here; the store-backed ones — job work
//! trees, job outputs, replica twins, release versions — run in [`store`]
//! with whatever scan share the first half left.

pub(crate) mod budget;
mod store;
pub(crate) mod summary;

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use crate::providers::local::disk_cleanup::janitor::state::error::JanitorError;
use crate::providers::local::disk_cleanup::janitor::state::report::CleanupReport;
use crate::providers::local::disk_cleanup::{
    backup_twins, build_caches, chromium_clones, hf, job_outputs, queue_workdirs, release_store,
    weles,
};
use crate::targets::DiskCleanupPolicy;

/// The cleaners that walk a filesystem, in the order one pass runs them.
///
/// `local_snapshots` is deliberately not here: it walks nothing, spends no
/// scan share, and has to run AFTER these, because what it recovers
/// is the blocks their deletions left pinned in a Time Machine snapshot.
pub(crate) const CLEANER_ORDER: [&str; 8] = [
    "huggingface_cache",
    "weles_recordings",
    "build_caches",
    chromium_clones::CLEANER,
    queue_workdirs::CLEANER,
    job_outputs::CLEANER,
    backup_twins::CLEANER,
    release_store::CLEANER,
];

/// Divide the remaining scan capacity between declared cleaners in
/// [`CLEANER_ORDER`]. Unspent capacity rolls forward to the cleaners behind;
/// the last declared cleaner receives whatever remains.
pub(super) struct Shares<'a> {
    policy: &'a DiskCleanupPolicy,
}

impl Shares<'_> {
    /// Declared cleaners still to run after `current`.
    pub(super) fn declared_after(&self, current: &str) -> i64 {
        CLEANER_ORDER
            .iter()
            .skip_while(|name| **name != current)
            .skip(1)
            .filter(|name| self.policy.cleaners.contains_key(**name))
            .count() as i64
    }

    /// One cleaner's item share of `remaining` with `behind` cleaners left.
    pub(super) fn share(&self, remaining: i64, behind: i64) -> i64 {
        if behind <= 0 {
            remaining
        } else {
            (remaining / (behind + 1)).max(1).min(remaining)
        }
    }
}

/// Run every declared cleaner inside its share of the pass's scan capacity.
///
/// Moved out of `run_with_lock` unchanged. `Err` carries exactly the error
/// the HuggingFace scan escaped with, which the caller records as `runtime`
/// and turns into `invalid_or_unavailable_policy`.
pub(crate) async fn run_cleaners(
    home: &Path,
    policy: &DiskCleanupPolicy,
    declared_release_versions: &BTreeMap<String, BTreeSet<String>>,
    attempted_at: f64,
    report: &mut CleanupReport,
) -> Result<(), JanitorError> {
    let shares = Shares { policy };
    let declared_after = |current: &str| shares.declared_after(current);
    let share = |remaining: i64, behind: i64| shares.share(remaining, behind);
    // Past every early return: from here the cleaner table is a measurement
    // this pass actually made, so the report may carry one.
    report.scanned = true;
    // Errors escaping _run_hf (a vanished cache root mid-pass, a failed
    // free-space probe) hit Python's outer `except BaseException`:
    // `runtime` error + the default outcome, state still written.
    hf::run_hf(
        home,
        policy,
        report.active_job_count,
        attempted_at,
        share(policy.max_scan_items, declared_after("huggingface_cache")),
        report,
    )?;
    let scanned = report.hf.scanned_items;
    let remaining_scan = (policy.max_scan_items - scanned).max(0);
    if remaining_scan == 0 && policy.cleaners.contains_key("weles_recordings") {
        report.caps.scan = true;
    }
    weles::scan_weles(
        home,
        policy,
        attempted_at,
        share(remaining_scan, declared_after("weles_recordings")),
        report,
    );
    let remaining_after_weles =
        (policy.max_scan_items - report.hf.scanned_items - report.weles.scanned_items).max(0);
    if remaining_after_weles == 0 && policy.cleaners.contains_key("build_caches") {
        report.caps.scan = true;
    }
    // A build-cache root can cover the whole home. Its cursor carries
    // unexamined directories into the next count-bounded pass.
    build_caches::scan_build_caches(
        home,
        policy,
        attempted_at,
        share(remaining_after_weles, declared_after("build_caches")),
        report.builds_cursor.take(),
        report,
    );
    let remaining_after_builds = (policy.max_scan_items
        - report.hf.scanned_items
        - report.weles.scanned_items
        - report.builds.scanned_items)
        .max(0);
    if remaining_after_builds == 0 && policy.cleaners.contains_key(chromium_clones::CLEANER) {
        report.caps.scan = true;
    }
    // The only cleaner whose root is outside this account's home: macOS puts
    // the clones in the per-user temporary container. Its unused scan share
    // rolls forward to the lifecycle cleaners behind it.
    chromium_clones::scan_chromium_clones(
        home,
        policy,
        attempted_at,
        share(
            remaining_after_builds,
            declared_after(chromium_clones::CLEANER),
        ),
        report,
    );
    let remaining_after_clones = (policy.max_scan_items
        - report.hf.scanned_items
        - report.weles.scanned_items
        - report.builds.scanned_items
        - report.clones.scanned_items)
        .max(0);
    store::run_store_cleaners(
        home,
        policy,
        declared_release_versions,
        attempted_at,
        remaining_after_clones,
        &shares,
        report,
    )
    .await;
    Ok(())
}
