//! The durable outputs of finished queue jobs.
//!
//! Every job the queue runs writes what it produced under
//! `status/<job_id>/output/` of the store: build logs, receipts, and the
//! release archive a publish step copies into `releases/` before the run
//! ends. Without this cleaner nothing ever removes those files.
//!
//! What a job's outputs are still for, and therefore what this cleaner keeps:
//!
//! - the outputs of a job the queue lists as live (`queue/`, `running/`): a
//!   job still writing them;
//! - the outputs of a job the queue does not positively list as terminal.
//!   "Not live" is not enough: a job id that appears nowhere in the queue
//!   store may be a record this host cannot see, and an unreadable store
//!   removes nothing;
//! - the small records inside `output/`: `receipt.json`, `scratch.json`, and
//!   every `*.log` and `*.json`. The register of publication attempts reads
//!   them (`stado release status` shows the failure text off the job's own
//!   log); what this cleaner reclaims is the payload beside them.
//!
//! Reclaim removes the payload files of one eligible job together and leaves
//! the job's directory, so a later reader finds the records and an honest
//! absence rather than a missing job.
//!
//! Layout: [`inventory`] names the candidate population from the `status/`
//! directory and asks the queue which of them are positively terminal; this
//! module owns the cleaner's name, its roots and the pass that writes the
//! report.

mod inventory;

use std::collections::BTreeSet;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

pub use inventory::candidate_job_ids;

use super::{euid, free_bytes, CleanupReport, JanitorError};

/// The name this cleaner's report is filed under.
pub const CLEANER: &str = "job_outputs";

/// The store prefix every job's durable output lives under.
const STATUS_PREFIX: &str = "status";

/// The directory inside one job's status prefix that holds its outputs.
pub(super) const OUTPUT_DIR: &str = "output";

/// Every directory the queue store's `status/` prefix maps to on this host.
///
/// The store root is the account's own, `~/.stado/local-storage`, the same
/// path `backup_twins` compares its replica against — not the configured
/// `WC_LOCAL_STORAGE_PATH`, which the janitor's service process does not
/// carry, so reading it would answer `root_absent` for bytes the replica
/// cleaner walks in the same pass.
///
/// Inside that root, the host serving the fleet's object API keeps each
/// namespace's keys under `ecosystem/<namespace>/` while a device-local
/// queue keeps them flat. Both layouts are walked, whichever exist, so no
/// namespace or backend setting has to be right for the bytes to be found.
pub fn status_roots(home: &Path) -> Vec<PathBuf> {
    let store = home.join(super::backup_twins::PRIMARY_ROOT);
    let mut roots = Vec::new();
    let flat = store.join(STATUS_PREFIX);
    if flat.is_dir() {
        roots.push(flat);
    }
    let Ok(namespaces) = std::fs::read_dir(store.join("ecosystem")) else {
        return roots;
    };
    for namespace in namespaces.flatten() {
        let served = namespace.path().join(STATUS_PREFIX);
        if served.is_dir() {
            roots.push(served);
        }
    }
    roots.sort();
    roots
}

/// Whether one entry directly inside `output/` is a record the register
/// reads, kept always, rather than a payload.
///
/// Only at that level. A job that wrote a tree under `output/` wrote
/// artifacts, whatever their extension: a crawl job's `.inst.json` under
/// `output/<run>/` can be over a hundred MB, and an extension rule that
/// reached into the tree would keep the very bytes this cleaner exists to
/// reclaim.
fn is_record(name: &str) -> bool {
    name.ends_with(".json") || name.ends_with(".log")
}

/// Reclaim the payload outputs of terminal jobs.
///
/// `terminal_jobs` is the set the queue positively listed as terminal among
/// the candidates; `None` means the queue store could not be read this pass,
/// and this cleaner then removes nothing at all.
pub fn scan_job_outputs(
    status_roots: &[PathBuf],
    home: &Path,
    enforcing: bool,
    terminal_jobs: Option<&BTreeSet<String>>,
    report: &mut CleanupReport,
) {
    let body = |report: &mut CleanupReport| -> Result<(), JanitorError> {
        if status_roots.is_empty() {
            report.skip_job_outputs("root_absent", 1);
            return Ok(());
        }
        let Some(terminal) = terminal_jobs else {
            report.skip_job_outputs("queue_store_unreadable", 1);
            return Ok(());
        };
        let home_device = std::fs::metadata(home)?.dev();
        for (root, job_id) in status_roots
            .iter()
            .flat_map(|root| terminal.iter().map(move |job_id| (root, job_id)))
        {
            let output = root.join(job_id).join(OUTPUT_DIR);
            if !output.is_dir() {
                report.skip_job_outputs("output_absent", 1);
                continue;
            }
            let mut pending = vec![(output, 0usize)];
            while let Some((directory, depth)) = pending.pop() {
                let Ok(entries) = std::fs::read_dir(&directory) else {
                    report.skip_job_outputs("unreadable_directory", 1);
                    continue;
                };
                for entry in entries.flatten() {
                    report.job_outputs.scanned_items += 1;
                    let name = entry.file_name().to_string_lossy().into_owned();
                    let path = entry.path();
                    let Ok(info) = std::fs::symlink_metadata(&path) else {
                        report.skip_job_outputs("stat_failed", 1);
                        continue;
                    };
                    if info.file_type().is_symlink()
                        || info.uid() != euid()
                        || info.dev() != home_device
                    {
                        report.skip_job_outputs("not_a_plain_owned_file", 1);
                        continue;
                    }
                    // A job that wrote a tree wrote artifacts; the walk
                    // reaches them and leaves the directories themselves,
                    // so a later reader sees the job's own shape.
                    if info.is_dir() {
                        pending.push((path, depth + 1));
                        continue;
                    }
                    if !info.is_file() {
                        report.keep_job_outputs("not_a_plain_owned_file", info.len() as i64);
                        continue;
                    }
                    if depth == 0 && is_record(&name) {
                        report.keep_job_outputs("record_kept", info.len() as i64);
                        continue;
                    }
                    report.job_outputs.eligible_items += 1;
                    report.job_outputs.expected_bytes +=
                        i64::try_from(info.len()).unwrap_or(i64::MAX);
                    if !enforcing {
                        continue;
                    }
                    let attempt = (|| -> Result<i64, JanitorError> {
                        let before = free_bytes(home)?;
                        std::fs::remove_file(&path)?;
                        Ok(free_bytes(home)? - before)
                    })();
                    match attempt {
                        Ok(delta) => {
                            report.job_outputs.actual_free_delta_bytes += delta.max(0);
                            report.job_outputs.deleted_items += 1;
                        }
                        Err(exc) => report.add_error(CLEANER, &exc),
                    }
                }
            }
        }
        Ok(())
    };
    if let Err(exc) = body(report) {
        report.add_error(CLEANER, &exc);
    }
}
