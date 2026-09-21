//! The fixed cleaner order and one pass's run of every declared cleaner.
//!
//! The rebuildable-cache cleaners run here; the store-backed ones — job work
//! trees, job outputs, replica twins, release versions — run in [`store`]
//! with whatever item and time share the first half left.

pub(crate) mod budget;
mod store;
pub(crate) mod summary;

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::time::Instant;

use crate::providers::local::disk_cleanup::janitor::state::error::JanitorError;
use crate::providers::local::disk_cleanup::janitor::state::report::CleanupReport;
use crate::providers::local::disk_cleanup::janitor::DEADLINE_SECONDS;
use crate::providers::local::disk_cleanup::{
    backup_twins, build_caches, chromium_clones, hf, job_outputs, queue_workdirs, release_store,
    weles,
};
use crate::targets::DiskCleanupPolicy;

/// The cleaners that walk a filesystem, in the order one pass runs them.
///
/// `local_snapshots` is deliberately not here: it walks nothing, spends no
/// item or time share, and has to run AFTER these, because what it recovers
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

/// How one pass divides its item and time budget between the declared
/// cleaners in [`CLEANER_ORDER`].
///
/// Every cleaner used to receive `max_scan_items` minus what the ones
/// before it had spent, which reads as fair and is not: the cleaners run in
/// a fixed order, and one whose root is large enough to exhaust the cap
/// takes the whole pass, every pass, forever. Measured on
/// charless-mac-mini on 2026-08-31 with `max_scan_items: 10000` and six
/// declared cleaners: `weles_recordings` scanned 15, `build_caches` scanned
/// 9,985 and found NOTHING eligible, and `chromium_clones`,
/// `queue_workdirs` and `backup_twins` each received a budget of zero and
/// scanned nothing — pass after pass, under real disk pressure, with 18 GiB
/// of proven duplicates sitting in the replica that `backup_twins` exists
/// to reclaim. The outcome was `cap_reached`, which is true and reads like
/// work being done.
///
/// An equal share of what is left, with everything unspent rolling forward
/// to the cleaners behind: a cleaner that scans less than its share leaves
/// more for the rest, and the last declared cleaner is handed whatever
/// remains. No cleaner is ever handed zero while it is declared, which is
/// the property that was missing. Item shares alone do not make the fixed
/// order fair: a cleaner can spend the whole wall-clock allowance while
/// staying inside its item share, so each declared cleaner also gets an
/// equal slice of the time that remains at the instant it starts; an unused
/// slice stays inside the single global deadline and rolls forward.
pub(super) struct Shares<'a> {
    policy: &'a DiskCleanupPolicy,
    pub(super) deadline: Instant,
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

    /// The instant `current` must stop by.
    pub(super) fn time_share(&self, current: &str) -> Instant {
        let now = Instant::now();
        let remaining = self.deadline.saturating_duration_since(now);
        let slots = self.declared_after(current).saturating_add(1) as u32;
        now + remaining / slots
    }
}

/// Run every declared cleaner inside its item and time share of the pass.
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
    // The host's declared pass budget, or this module's own 30 seconds when it
    // declares none. This is the limit that actually decides how much of a
    // large tree one pass sees: on `lukasz-macbook` `max_scan_items` never
    // bound and the deadline did, every pass.
    let pass_seconds = policy
        .max_pass_seconds
        .filter(|seconds| *seconds > 0)
        .map_or(DEADLINE_SECONDS, |seconds| seconds as f64);
    let deadline = Instant::now() + std::time::Duration::from_secs_f64(pass_seconds);
    let shares = Shares { policy, deadline };
    let declared_after = |current: &str| shares.declared_after(current);
    let share = |remaining: i64, behind: i64| shares.share(remaining, behind);
    let time_share = |current: &str| shares.time_share(current);
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
        time_share("huggingface_cache"),
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
        time_share("weles_recordings"),
        report,
    );
    let remaining_after_weles =
        (policy.max_scan_items - report.hf.scanned_items - report.weles.scanned_items).max(0);
    if remaining_after_weles == 0 && policy.cleaners.contains_key("build_caches") {
        report.caps.scan = true;
    }
    // The build-cache scan is the only one whose root can be the whole of
    // `$HOME`: it walks with its item and time shares of what the fixed-layout
    // cleaners left. It is also the only one that cannot finish in one pass on
    // a large tree, so it resumes from where the last pass stopped instead of
    // restarting.
    build_caches::scan_build_caches(
        home,
        policy,
        attempted_at,
        share(remaining_after_weles, declared_after("build_caches")),
        time_share("build_caches"),
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
    // the clones in the per-user temporary container. Its unused item and time
    // shares roll forward to the lifecycle cleaners behind it.
    chromium_clones::scan_chromium_clones(
        home,
        policy,
        attempted_at,
        share(
            remaining_after_builds,
            declared_after(chromium_clones::CLEANER),
        ),
        time_share(chromium_clones::CLEANER),
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
