//! The second half of one pass: the cleaners that ask the queue store or
//! the release register before they delete — job work trees, job outputs,
//! replica twins and release versions — in the fixed order after the
//! rebuildable-cache cleaners have spent their shares.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::time::Instant;

use crate::providers::local::disk_cleanup::janitor::pass::once::keep_list::{
    fetch_live_job_ids, fetch_terminal_job_ids,
};
use crate::providers::local::disk_cleanup::janitor::state::report::CleanupReport;
use crate::providers::local::disk_cleanup::janitor::KEEP_LIST_BUDGET;
use crate::providers::local::disk_cleanup::{
    backup_twins, job_outputs, queue_workdirs, release_store,
};
use crate::targets::DiskCleanupPolicy;

use super::Shares;

/// Run the store-backed cleaners with what the cache cleaners left.
pub(super) async fn run_store_cleaners(
    home: &Path,
    policy: &DiskCleanupPolicy,
    declared_release_versions: &BTreeMap<String, BTreeSet<String>>,
    attempted_at: f64,
    remaining_after_clones: i64,
    shares: &Shares<'_>,
    report: &mut CleanupReport,
) {
    if remaining_after_clones == 0 && policy.cleaners.contains_key(queue_workdirs::CLEANER) {
        report.caps.scan = true;
    }
    // The queue's own per-job trees, after rebuildable caches.
    // Candidate names are captured under this same janitor lock. The queue
    // authority then downloads bodies only for candidates whose names overlap
    // live and terminal prefixes; everything unreadable remains on the keep
    // set, and any store failure disables this cleaner for the whole pass.
    let workdir_budget = shares.share(
        remaining_after_clones,
        shares.declared_after(queue_workdirs::CLEANER),
    );
    let workdir_deadline = shares.time_share(queue_workdirs::CLEANER);
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
    let remaining_after_workdirs = (remaining_after_clones - report.workdirs.scanned_items).max(0);
    if remaining_after_workdirs == 0 && policy.cleaners.contains_key(job_outputs::CLEANER) {
        report.caps.scan = true;
    }
    // The durable outputs of finished jobs. Its candidates are the job ids
    // with an output directory in the store; the queue is asked which of
    // exactly those it has retired, and only those are touched.
    let outputs_budget = shares.share(
        remaining_after_workdirs,
        shares.declared_after(job_outputs::CLEANER),
    );
    let outputs_deadline = shares.time_share(job_outputs::CLEANER);
    let status_roots = job_outputs::status_roots(
        home,
        policy
            .cleaners
            .get(job_outputs::CLEANER)
            .and_then(|cleaner| cleaner.root.as_deref()),
    );
    let terminal_jobs = if outputs_budget > 0 && policy.cleaners.contains_key(job_outputs::CLEANER)
    {
        match job_outputs::candidate_job_ids(&status_roots, outputs_budget, outputs_deadline) {
            Ok(candidates) if candidates.is_empty() => Some(BTreeSet::new()),
            Ok(candidates) => {
                let budget = outputs_deadline
                    .saturating_duration_since(Instant::now())
                    .min(KEEP_LIST_BUDGET);
                let wait = Instant::now();
                let ids = fetch_terminal_job_ids(&candidates, budget).await;
                report.store_wait_ms = report
                    .store_wait_ms
                    .saturating_add(wait.elapsed().as_millis().min(i64::MAX as u128) as i64);
                ids
            }
            Err(error) => {
                report.add_error(job_outputs::CLEANER, &error);
                None
            }
        }
    } else {
        Some(BTreeSet::new())
    };
    job_outputs::scan_job_outputs(
        &status_roots,
        home,
        policy,
        attempted_at,
        outputs_budget,
        outputs_deadline,
        terminal_jobs.as_ref(),
        report,
    );
    let remaining_after_outputs =
        (remaining_after_workdirs - report.job_outputs.scanned_items).max(0);
    if remaining_after_outputs == 0 && policy.cleaners.contains_key(backup_twins::CLEANER) {
        report.caps.scan = true;
    }
    // The disaster-recovery replica's proven duplicates. It is the only
    // cleaner here that has to READ the bytes it deletes: every object it
    // removes is hashed against the primary in this same pass. Release-store
    // cleanup remains behind it and receives its own item/time share.
    let twins_budget = shares.share(
        remaining_after_outputs,
        shares.declared_after(backup_twins::CLEANER),
    );
    backup_twins::scan_backup_twins(
        home,
        policy,
        crate::config::wc_stado_storage_namespace(),
        twins_budget,
        shares.time_share(backup_twins::CLEANER),
        report,
    );
    let remaining_after_twins =
        (remaining_after_outputs - report.backup_twins.scanned_items).max(0);
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
        shares.deadline,
        report,
    );
}
