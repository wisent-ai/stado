//! The fixed cleaner order and one pass's run of every declared cleaner.

pub(crate) mod budget;
pub(crate) mod summary;

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::time::Instant;

use crate::providers::local::disk_cleanup::janitor::pass::once::keep_list::fetch_live_job_ids;
use crate::providers::local::disk_cleanup::janitor::state::error::JanitorError;
use crate::providers::local::disk_cleanup::janitor::state::report::CleanupReport;
use crate::providers::local::disk_cleanup::janitor::{DEADLINE_SECONDS, KEEP_LIST_BUDGET};
use crate::providers::local::disk_cleanup::{
    backup_twins, build_caches, chromium_clones, hf, queue_workdirs, release_store, weles,
};
use crate::targets::DiskCleanupPolicy;

pub(crate) const CLEANER_ORDER: [&str; 7] = [
    "huggingface_cache",
    "weles_recordings",
    "build_caches",
    chromium_clones::CLEANER,
    queue_workdirs::CLEANER,
    backup_twins::CLEANER,
    release_store::CLEANER,
];

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
    // How much of the pass's remaining scan budget one cleaner may spend while
    // declared cleaners behind it have not run yet.
    //
    // Every cleaner used to receive `max_scan_items` minus what the ones
    // before it had spent, which reads as fair and is not: the cleaners run in
    // a fixed order, and one whose root is large enough to exhaust the cap
    // takes the whole pass, every pass, forever. Measured on
    // charless-mac-mini on 2026-08-31 with `max_scan_items: 10000` and six
    // declared cleaners: `weles_recordings` scanned 15, `build_caches` scanned
    // 9,985 and found NOTHING eligible, and `chromium_clones`,
    // `queue_workdirs` and `backup_twins` each received a budget of zero and
    // scanned nothing — pass after pass, under real disk pressure, with 18 GiB
    // of proven duplicates sitting in the replica that `backup_twins` exists
    // to reclaim. The outcome was `cap_reached`, which is true and reads like
    // work being done.
    //
    // An equal share of what is left, with everything unspent rolling forward
    // to the cleaners behind: a cleaner that scans less than its share leaves
    // more for the rest, and the last declared cleaner is handed whatever
    // remains. No cleaner is ever handed zero while it is declared, which is
    // the property that was missing.
    let declared_after = |current: &str| -> i64 {
        CLEANER_ORDER
            .iter()
            .skip_while(|name| **name != current)
            .skip(1)
            .filter(|name| policy.cleaners.contains_key(**name))
            .count() as i64
    };
    let share = |remaining: i64, behind: i64| -> i64 {
        if behind <= 0 {
            remaining
        } else {
            (remaining / (behind + 1)).max(1).min(remaining)
        }
    };
    // Item shares alone do not make the fixed order fair: a cleaner can spend
    // the whole wall-clock allowance while staying inside its item share. Give
    // each declared cleaner an equal slice of the time that remains at the
    // instant it starts. Any unused slice stays inside the single global
    // deadline and is therefore rolled into the next cleaner's calculation.
    let time_share = |current: &str| -> Instant {
        let now = Instant::now();
        let remaining = deadline.saturating_duration_since(now);
        let slots = declared_after(current).saturating_add(1) as u32;
        now + remaining / slots
    };
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
    if remaining_after_clones == 0 && policy.cleaners.contains_key(queue_workdirs::CLEANER) {
        report.caps.scan = true;
    }
    // The queue's own per-job trees, after rebuildable caches.
    // Candidate names are captured under this same janitor lock. The queue
    // authority then downloads bodies only for candidates whose names overlap
    // live and terminal prefixes; everything unreadable remains on the keep
    // set, and any store failure disables this cleaner for the whole pass.
    let workdir_budget = share(
        remaining_after_clones,
        declared_after(queue_workdirs::CLEANER),
    );
    let workdir_deadline = time_share(queue_workdirs::CLEANER);
    let live_jobs = if workdir_budget > 0 && policy.cleaners.contains_key(queue_workdirs::CLEANER) {
        match queue_workdirs::candidate_job_ids(
            home,
            policy
                .cleaners
                .get(queue_workdirs::CLEANER)
                .and_then(|cleaner| cleaner.root.as_deref()),
            workdir_budget,
            workdir_deadline,
        ) {
            Ok(candidates) => {
                let budget = workdir_deadline
                    .saturating_duration_since(Instant::now())
                    .min(KEEP_LIST_BUDGET);
                let wait = Instant::now();
                let ids = fetch_live_job_ids(&candidates, budget).await;
                report.store_wait_ms = report
                    .store_wait_ms
                    .saturating_add(wait.elapsed().as_millis().min(i64::MAX as u128) as i64);
                ids
            }
            Err(error) => {
                report.add_error(queue_workdirs::CLEANER, &error);
                None
            }
        }
    } else {
        Some(Vec::new())
    };
    queue_workdirs::scan_queue_workdirs(
        home,
        policy,
        attempted_at,
        workdir_budget,
        workdir_deadline,
        live_jobs.as_deref(),
        report,
    );
    let remaining_after_workdirs = (policy.max_scan_items
        - report.hf.scanned_items
        - report.weles.scanned_items
        - report.builds.scanned_items
        - report.clones.scanned_items
        - report.workdirs.scanned_items)
        .max(0);
    if remaining_after_workdirs == 0 && policy.cleaners.contains_key(backup_twins::CLEANER) {
        report.caps.scan = true;
    }
    // The disaster-recovery replica's proven duplicates. It is the only
    // cleaner here that has to READ the bytes it deletes: every object it
    // removes is hashed against the primary in this same pass. Release-store
    // cleanup remains behind it and receives its own item/time share.
    let twins_budget = share(
        remaining_after_workdirs,
        declared_after(backup_twins::CLEANER),
    );
    backup_twins::scan_backup_twins(
        home,
        policy,
        crate::config::wc_stado_storage_namespace(),
        twins_budget,
        time_share(backup_twins::CLEANER),
        report,
    );
    let remaining_after_twins = (policy.max_scan_items
        - report.hf.scanned_items
        - report.weles.scanned_items
        - report.builds.scanned_items
        - report.clones.scanned_items
        - report.workdirs.scanned_items
        - report.backup_twins.scanned_items)
        .max(0);
    if remaining_after_twins == 0 && policy.cleaners.contains_key(release_store::CLEANER) {
        report.caps.scan = true;
    }
    // Immutable release versions nothing on this host still names, scanned
    // last: it deletes whole version directories, so it takes the smallest
    // share and only after every cleaner that reclaims scratch has had its
    // turn — a release is the one class here that costs a rebuild to get
    // back.
    release_store::scan_release_store(
        home,
        policy,
        declared_release_versions,
        remaining_after_twins,
        deadline,
        report,
    );
    Ok(())
}
