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

use super::janitor::state::report::met::Met;
use super::{euid, free_bytes, CleanupReport, JanitorError};

/// The name this cleaner's report is filed under.
pub const CLEANER: &str = "job_outputs";

/// The store prefix every job's durable output lives under.
const STATUS_PREFIX: &str = "status";

/// The directory inside one job's status prefix that holds its outputs.
pub(super) const OUTPUT_DIR: &str = "output";

/// Every directory the queue store's `status/` prefix maps to on this host.
///
/// The store root is the account's own, the product's declared default
/// [`crate::config_file::LOCAL_STORAGE_ROOT`] the replica cleaner compares against — not the configured
/// `WC_LOCAL_STORAGE_PATH`, which the janitor's service process does not
/// carry, so reading it would answer `root_absent` for bytes the replica
/// cleaner walks in the same pass.
///
/// Inside that root, the host serving the fleet's object API keeps each
/// namespace's keys under `ecosystem/<namespace>/` while a device-local
/// queue keeps them flat. Both layouts are walked, whichever exist, so no
/// namespace or backend setting has to be right for the bytes to be found.
pub fn status_roots(home: &Path) -> Vec<PathBuf> {
    let store = home.join(crate::config_file::LOCAL_STORAGE_ROOT);
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
    let mut met = Met::default();
    let body = |met: &mut Met, report: &mut CleanupReport| -> Result<(), JanitorError> {
        if status_roots.is_empty() {
            met.skipped("root_absent");
            return Ok(());
        }
        let Some(terminal) = terminal_jobs else {
            met.skipped("queue_store_unreadable");
            return Ok(());
        };
        let home_device = std::fs::metadata(home)?.dev();
        for (root, job_id) in status_roots
            .iter()
            .flat_map(|root| terminal.iter().map(move |job_id| (root, job_id)))
        {
            let output = root.join(job_id).join(OUTPUT_DIR);
            if !output.is_dir() {
                met.skipped("output_absent");
                continue;
            }
            // Whether a directory is the job's output directory itself, whose
            // record files are kept, or a tree the job wrote below it.
            let mut pending = vec![(output, true)];
            while let Some((directory, top)) = pending.pop() {
                let Ok(entries) = std::fs::read_dir(&directory) else {
                    met.skipped("unreadable_directory");
                    continue;
                };
                let entries: Vec<_> = entries.collect();
                met.listed(entries.len());
                for entry in entries {
                    // An entry the directory cannot list is a skipped item
                    // with its reason, never left out without a trace.
                    let Ok(entry) = entry else {
                        met.skipped("entry_unreadable");
                        continue;
                    };
                    let name = entry.file_name().to_string_lossy().into_owned();
                    let path = entry.path();
                    let Ok(info) = std::fs::symlink_metadata(&path) else {
                        met.skipped("stat_failed");
                        continue;
                    };
                    if info.file_type().is_symlink()
                        || info.uid() != euid()
                        || info.dev() != home_device
                    {
                        met.skipped("not_a_plain_owned_file");
                        continue;
                    }
                    // A job that wrote a tree wrote artifacts; the walk
                    // reaches them and leaves the directories themselves,
                    // so a later reader sees the job's own shape.
                    if info.is_dir() {
                        pending.push((path, false));
                        continue;
                    }
                    if !info.is_file() {
                        met.kept("not_a_plain_owned_file", info.len());
                        continue;
                    }
                    if top && is_record(&name) {
                        met.kept("record_kept", info.len());
                        continue;
                    }
                    met.eligible(info.len());
                    if !enforcing {
                        continue;
                    }
                    let attempt = (|| -> Result<i64, JanitorError> {
                        let before = free_bytes(home)?;
                        std::fs::remove_file(&path)?;
                        Ok(free_bytes(home)? - before)
                    })();
                    match attempt {
                        // Free space another writer took meanwhile is no
                        // space this removal freed: only a gain is counted.
                        Ok(delta) => met.removed(delta.is_positive().then(|| delta.unsigned_abs())),
                        Err(exc) => report.add_error(CLEANER, &exc),
                    }
                }
            }
        }
        Ok(())
    };
    if let Err(exc) = body(&mut met, report) {
        report.add_error(CLEANER, &exc);
    }
    match met.report() {
        Ok(record) => report.job_outputs = record,
        Err(cause) => report.add_error(CLEANER, &JanitorError::os(&cause)),
    }
}
