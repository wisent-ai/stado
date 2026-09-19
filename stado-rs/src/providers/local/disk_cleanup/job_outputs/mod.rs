//! Retention for the durable outputs of finished queue jobs.
//!
//! Every job the queue runs writes what it produced under
//! `status/<job_id>/output/` of the store: build logs, receipts, and the
//! release archive a publish step copies into `releases/` before the run
//! ends. Nothing ever removed those files. Measured on `charless-mac-mini`
//! on 2026-09-19: `local-storage/ecosystem/probierz/status` held 12.1 GiB
//! across 6,243 objects, the oldest from a crawl job of 2026-08-16, while
//! the disk sat at 1.8 GiB free against an 8 GiB watermark, every declared
//! cleaner reported nothing eligible, and the host refused to roll out the
//! Brama release the fleet needed — the janitor could not name the bytes.
//!
//! What a job's outputs are still for, and therefore what this cleaner keeps:
//!
//! - the outputs of a job the queue lists as live (`queue/`, `running/`): a
//!   job still writing them;
//! - the outputs of a job the queue does not positively list as terminal.
//!   "Not live" is not enough: a job id that appears nowhere in the queue
//!   store may be a record this host cannot see, and an unreadable store
//!   removes nothing;
//! - the outputs younger than the policy's `min_age_seconds`, whose floor is
//!   seven days: `stado release resume` and `stado release status` read a
//!   terminal job's receipt and log back for that long after it ends, and a
//!   publish that failed after its build can still be resumed from the
//!   built archive within it;
//! - the small records inside `output/`: `receipt.json`, `scratch.json`, and
//!   every `*.log` and `*.json`. The register of publication attempts reads
//!   them (`stado release status` shows the failure text off the job's own
//!   log); what this cleaner reclaims is the payload beside them.
//!
//! Reclaim removes the payload files of one eligible job together and leaves
//! the job's directory, so a later reader finds the records and an honest
//! absence rather than a missing job.
//!
//! Layout: [`inventory`] names the bounded candidate population from the
//! `status/` directory and asks the queue which of them are positively
//! terminal; this module owns the cleaner's name, its roots and the pass that
//! spends the budget and writes the report.

mod inventory;

use std::collections::BTreeSet;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::Instant;

pub use inventory::candidate_job_ids;

use super::{euid, free_bytes, CleanupReport, JanitorError, GIB};
use crate::targets::DiskCleanupPolicy;

/// The cleaner's registry name, and the key its counts appear under in the
/// janitor's report. Declared here rather than spelled at each use, because
/// [`crate::targets`]'s allowed-cleaner list, the report and this scan have to
/// name the same cleaner or a policy authorizes a pass that never runs.
pub const CLEANER: &str = "job_outputs";

/// The store prefix every job's durable output lives under.
const STATUS_PREFIX: &str = "status";

/// The directory inside one job's status prefix that holds its outputs.
pub(super) const OUTPUT_DIR: &str = "output";

/// Every directory the queue store's `status/` prefix maps to on this host.
///
/// The host that serves the fleet's object API keeps each namespace's keys
/// under `ecosystem/<namespace>/` of the local store — the layout the
/// `backup_twins` cleaner compares its replica against — while a queue on
/// the device-local backend keeps its keys directly under the configured
/// path. Neither the backend setting nor the namespace is reliably in the
/// janitor's own process: on charless-mac-mini on 2026-09-19 it answered
/// `root_absent` for 12 GiB that was on the disk, twice, once per reading
/// it tried. The layout on disk is the answer that needs no configuration:
/// the flat prefix and every namespace's, whichever exist.
pub fn status_roots(local_storage_path: &Path) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    let flat = local_storage_path.join(STATUS_PREFIX);
    if flat.is_dir() {
        roots.push(flat);
    }
    let Ok(namespaces) = std::fs::read_dir(local_storage_path.join("ecosystem")) else {
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

/// Whether one output file is a record the register reads, kept regardless
/// of age, rather than a payload.
fn is_record(name: &str) -> bool {
    name.ends_with(".json") || name.ends_with(".log")
}

/// Reclaim the payload outputs of terminal jobs older than the policy's age.
///
/// `terminal_jobs` is the set the queue positively listed as terminal among
/// the candidates; `None` means the queue store could not be read this pass,
/// and this cleaner then removes nothing at all.
#[allow(clippy::too_many_arguments)]
pub fn scan_job_outputs(
    status_roots: &[PathBuf],
    home: &Path,
    policy: &DiskCleanupPolicy,
    now: f64,
    remaining_scan: i64,
    deadline: Instant,
    terminal_jobs: Option<&BTreeSet<String>>,
    report: &mut CleanupReport,
) {
    let Some(configured) = policy.cleaners.get(CLEANER) else {
        return;
    };
    if remaining_scan <= 0 {
        return;
    }
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
        let min_age = configured.min_age_seconds.max(0) as f64;
        let mut deleted_bytes = 0i64;
        let mut budget = remaining_scan;
        for (root, job_id) in status_roots
            .iter()
            .flat_map(|root| terminal.iter().map(move |job_id| (root, job_id)))
        {
            if Instant::now() >= deadline {
                report.caps.deadline = true;
                report.skip_job_outputs("scan_deadline", 1);
                break;
            }
            let output = root.join(job_id).join(OUTPUT_DIR);
            let entries = match std::fs::read_dir(&output) {
                Ok(entries) => entries,
                Err(_) => {
                    report.skip_job_outputs("output_absent", 1);
                    continue;
                }
            };
            for entry in entries.flatten() {
                budget -= 1;
                if budget < 0 {
                    report.caps.scan = true;
                    report.skip_job_outputs("scan_cap", 1);
                    return Ok(());
                }
                report.job_outputs.scanned_items += 1;
                let name = entry.file_name().to_string_lossy().into_owned();
                let path = entry.path();
                let Ok(info) = std::fs::symlink_metadata(&path) else {
                    report.skip_job_outputs("stat_failed", 1);
                    continue;
                };
                if !info.is_file()
                    || info.file_type().is_symlink()
                    || info.uid() != euid()
                    || info.dev() != home_device
                {
                    report.skip_job_outputs("not_a_plain_owned_file", 1);
                    continue;
                }
                if is_record(&name) {
                    report.skip_job_outputs("record_kept", 1);
                    continue;
                }
                if now - (info.mtime() as f64) < min_age {
                    report.skip_job_outputs("younger_than_min_age", 1);
                    continue;
                }
                report.job_outputs.eligible_items += 1;
                let expected = i64::try_from(info.len()).unwrap_or(i64::MAX);
                report.job_outputs.expected_bytes += expected;
                if policy.mode != "enforce" {
                    continue;
                }
                if report.job_outputs.deleted_items >= policy.max_items_per_pass {
                    report.caps.items = true;
                    report.skip_job_outputs("item_cap", 1);
                    continue;
                }
                if deleted_bytes >= policy.max_bytes_per_pass {
                    report.caps.bytes = true;
                    report.skip_job_outputs("byte_cap", 1);
                    continue;
                }
                if free_bytes(home)? >= policy.target_free_gb * GIB {
                    return Ok(());
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
                        deleted_bytes += expected;
                    }
                    Err(exc) => report.add_error(CLEANER, &exc),
                }
            }
        }
        Ok(())
    };
    if let Err(exc) = body(report) {
        report.add_error(CLEANER, &exc);
    }
}
