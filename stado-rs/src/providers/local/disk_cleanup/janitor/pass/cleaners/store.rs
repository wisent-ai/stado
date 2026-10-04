//! The second half of one pass: the cleaners that ask the queue store or
//! the release register before they delete — job work trees, job outputs,
//! replica twins and release versions.

use std::collections::BTreeSet;
use std::path::Path;
use std::time::Instant;

use crate::providers::local::disk_cleanup::janitor::pass::once::keep_list::{
    fetch_live_job_ids, fetch_terminal_job_ids,
};
use crate::providers::local::disk_cleanup::janitor::state::report::CleanupReport;
use crate::providers::local::disk_cleanup::{
    backup_twins, job_outputs, queue_workdirs, release_store,
};

use super::PassInputs;

/// Add the time spent waiting on the queue store to the report.
fn charge_wait(report: &mut CleanupReport, wait: Instant) {
    report.store_wait_ms = report
        .store_wait_ms
        .saturating_add(wait.elapsed().as_millis().min(i64::MAX as u128) as i64);
}

/// Run the store-backed cleaners.
pub(super) async fn run_store_cleaners(
    home: &Path,
    inputs: &PassInputs<'_>,
    report: &mut CleanupReport,
) {
    let enforcing = inputs.enforcing;
    // The queue's own per-job trees. Candidate names are captured under this
    // same janitor lock. The queue authority then downloads bodies only for
    // candidates whose names overlap live and terminal prefixes; everything
    // unreadable remains on the keep set, and any store failure disables this
    // cleaner for the whole pass.
    let live_jobs = match queue_workdirs::candidate_job_ids(home) {
        Ok(candidates) => {
            let wait = Instant::now();
            let ids = match fetch_live_job_ids(&candidates).await {
                Ok(ids) => Some(ids),
                Err(error) => {
                    report.add_error(queue_workdirs::CLEANER, &error);
                    None
                }
            };
            charge_wait(report, wait);
            ids
        }
        Err(error) => {
            report.add_error(queue_workdirs::CLEANER, &error);
            None
        }
    };
    queue_workdirs::scan_queue_workdirs(home, enforcing, live_jobs.as_deref(), report);
    // The durable outputs of finished jobs. Its candidates are the job ids
    // with an output directory in the store; the queue is asked which of
    // exactly those it has retired, and only those are touched.
    let status_roots = job_outputs::status_roots(home);
    let terminal_jobs = match job_outputs::candidate_job_ids(&status_roots) {
        Ok(candidates) if candidates.is_empty() => Some(BTreeSet::new()),
        Ok(candidates) => {
            let wait = Instant::now();
            let ids = match fetch_terminal_job_ids(&candidates).await {
                Ok(ids) => Some(ids),
                Err(error) => {
                    report.add_error(job_outputs::CLEANER, &error);
                    None
                }
            };
            charge_wait(report, wait);
            ids
        }
        Err(error) => {
            report.add_error(job_outputs::CLEANER, &error);
            None
        }
    };
    job_outputs::scan_job_outputs(
        &status_roots,
        home,
        enforcing,
        terminal_jobs.as_ref(),
        report,
    );
    // The disaster-recovery replica's proven duplicates. It is the only
    // cleaner here that has to READ the bytes it deletes: every object it
    // removes is hashed against the primary in this same pass.
    backup_twins::scan_backup_twins(
        home,
        crate::config::wc_stado_storage_namespace(),
        enforcing,
        report,
    );
    // Immutable release versions nothing on this host or in the fleet still
    // names, after every cleaner that reclaims scratch: a release is the one
    // class here that costs a rebuild to get back.
    release_store::scan_release_store(home, enforcing, inputs.declared_release_versions, report);
}
