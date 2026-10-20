//! The second half of one pass: the cleaners that ask the queue store or
//! the release register before they delete — job work trees, job outputs,
//! replica twins and release versions. The queue was asked before the
//! exclusive lock ([`super::asked`]); these act on its answers.

use std::path::Path;

use crate::providers::local::space_reclaim::janitor::state::report::CleanupReport;
use crate::providers::local::space_reclaim::{
    backup_twins, job_outputs, queue_workdirs, release_store,
};

use super::PassInputs;

/// Run the store-backed cleaners.
pub(super) async fn run_store_cleaners(
    home: &Path,
    inputs: &PassInputs<'_>,
    report: &mut CleanupReport,
) {
    let enforcing = inputs.enforcing;
    // The queue's own per-job trees. Candidate names are read again under
    // this lock; one that was not on disk when the queue was asked is kept
    // as live. A candidate list or queue answer that could not be read
    // disables this cleaner for the whole pass.
    let live_jobs = match queue_workdirs::candidate_job_ids(home) {
        Ok(now) => inputs.asked.live_workdirs(&now),
        Err(error) => {
            report.add_error(queue_workdirs::CLEANER, &error);
            None
        }
    };
    queue_workdirs::scan_queue_workdirs(home, enforcing, live_jobs.as_deref(), report);
    // The durable outputs of finished jobs: only the ones the queue named
    // terminal are touched.
    let status_roots = job_outputs::status_roots(home);
    job_outputs::scan_job_outputs(
        &status_roots,
        home,
        enforcing,
        inputs.asked.terminal_outputs.as_ref(),
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
