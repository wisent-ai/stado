//! Terminal queue-job workdir cleanup: eviction of the per-job scratch trees
//! the local agent creates at `/tmp/wc-<job_id>` and never returns to.
//!
//! NO Python original. Measured on `charless-mac-mini` on 2026-08-30, and the
//! measurement is the whole argument for this cleaner. That host had just been
//! put back on the fleet store after publishing capacity into a device-local
//! one, its queue began draining for the first time in seven days, and its free
//! space fell 19.3 -> 17.0 -> 13.8 GiB in about forty minutes. At 13.8 GiB it
//! was under the registry's 15 GiB low watermark, so the agent stopped claiming
//! and the queue stalled at 52 jobs with nothing running.
//!
//! Every declared cleaner reported zero eligible items throughout, because the
//! policy declared `huggingface_cache` and `weles_recordings` and the bytes were
//! in neither: the queued work is `cargo run --release -- crawl-*`, and a Rust
//! build's `target/` lands in the job's own workdir. `stado host reclaim`
//! recovered 4.3 GiB from eight of those workdirs in one pass — more than the
//! 1.2 GiB needed to clear the watermark — but that command runs when an
//! operator types it, and the janitor, which runs every 300s and is what keeps
//! an unattended host above its watermark, could not reach a byte of it.
//!
//! So the shape of the defect is not the disk. It is a janitor whose policy
//! cannot reach what actually fills the machine it guards: the host would
//! re-enter the same stall the next time the queue drained, and these jobs are
//! builds, so it always will.
//!
//! **Age is not the gate here, and that is deliberate.** The workdirs that
//! filled the disk were minutes old, so the day-long floor the build-cache,
//! weles and clone cleaners carry would leave this cleaner unable to touch the
//! only thing it exists for. What makes a workdir safe to remove is the state
//! of its job, not its age, and [`crate::deploy::host_reclaim`] already
//! established that rule for the same directories: a job that is neither queued
//! nor running is terminal, and a terminal job never returns to its workdir.
//! This cleaner consumes the same keep-list, built from the same two states, and
//! fails closed the same way — an unreadable queue store removes nothing rather
//! than guessing, because an empty keep-list would otherwise authorize deleting
//! every workdir on the host including the running job's.
//!
//! Operations are path-based, as in [`super::weles`] and
//! [`super::chromium_clones`]: shallow entries under a fixed root whose names
//! the agent itself chose, so the ordered refusals below plus the parent check
//! are the safety gate, and the tree walk and the removal are that module's —
//! imported, not copied.

use std::ffi::OsString;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::Instant;

use super::weles::{dir_size, remove_tree};
use super::{euid, free_bytes, CleanupReport, JanitorError, GIB};
use crate::targets::DiskCleanupPolicy;

/// The cleaner's registry name, and the key its counts appear under in the
/// janitor's report. Declared here rather than spelled at each use, because
/// [`crate::targets`]'s allowed-cleaner list, the report, and this scan have to
/// name the same cleaner or a policy authorizes a pass that never runs.
pub const CLEANER: &str = "queue_workdirs";

/// The prefix the local agent gives every job workdir it creates
/// (`/tmp/wc-<job_id>`, spelled in [`crate::providers::local::slots`]).
pub const WORKDIR_PREFIX: &str = "wc-";

/// The bootstrap scratch prefix that shares the same root and the same
/// lifetime, kept identical to the set `host reclaim`'s stage names so the
/// operator command and the unattended janitor cannot disagree about what a
/// terminal workdir is.
pub const BOOTSTRAP_PREFIX: &str = "stado-bootstrap-";

/// The roots job workdirs are created in: `/tmp` first, because
/// [`crate::providers::local::slots`] hardcodes it, then `$TMPDIR` when the
/// account has a different one, which is the pair `host reclaim` scans.
fn roots() -> Vec<PathBuf> {
    let mut roots = vec![PathBuf::from("/tmp")];
    if let Some(tmpdir) = std::env::var_os("TMPDIR") {
        let tmpdir = PathBuf::from(tmpdir);
        let normalized = tmpdir.to_string_lossy().trim_end_matches('/').to_string();
        if !normalized.is_empty() && normalized != "/tmp" {
            roots.push(PathBuf::from(normalized));
        }
    }
    roots
}

/// The job id a workdir belongs to, or `None` when the name is not one of the
/// two prefixes this cleaner owns.
///
/// Bootstrap scratch carries no job id and is terminal by construction: nothing
/// resumes a bootstrap, so it is judged by the policy's minimum age alone.
fn job_id(name: &str) -> Option<&str> {
    name.strip_prefix(WORKDIR_PREFIX)
}

/// Evict the workdirs of terminal jobs, oldest scan order first.
///
/// `live_jobs` is the keep-list: every job id currently in `queue` or `running`.
/// `None` means the queue store could not be read this pass, and this cleaner
/// then removes nothing at all.
pub fn scan_queue_workdirs(
    home: &Path,
    policy: &DiskCleanupPolicy,
    now: f64,
    remaining_scan: i64,
    deadline: Instant,
    live_jobs: Option<&[String]>,
    report: &mut CleanupReport,
) {
    let Some(configured) = policy.cleaners.get(CLEANER) else {
        return;
    };
    if remaining_scan <= 0 {
        return;
    }
    let body = |report: &mut CleanupReport| -> Result<(), JanitorError> {
        // Without the keep-list there is no terminal-job gate, and this cleaner
        // does not delete with a gate missing. The same refusal
        // `host reclaim` makes, for the same reason: an empty keep-list keeps
        // nothing, so failing open here would delete the running job's tree.
        let Some(live_jobs) = live_jobs else {
            report.skip_workdirs("queue_store_unreadable", 1);
            return Ok(());
        };
        let scan_roots = match &configured.root {
            Some(configured_root) => vec![crate::config_file::expand_tilde(configured_root)],
            None => roots(),
        };
        let home_device = std::fs::metadata(home)?.dev();
        let mut ordered: Vec<(OsString, PathBuf)> = Vec::new();
        let mut budget = remaining_scan;
        for root in &scan_roots {
            if !root.is_dir() {
                report.skip_workdirs("root_absent", 1);
                continue;
            }
            let entries = std::fs::read_dir(root)?;
            for entry in entries {
                let entry = entry?;
                let name = entry.file_name();
                let text = name.to_string_lossy();
                if !text.starts_with(WORKDIR_PREFIX) && !text.starts_with(BOOTSTRAP_PREFIX) {
                    continue;
                }
                ordered.push((name, entry.path()));
                budget -= 1;
                if budget <= 0 {
                    report.caps.scan = true;
                    report.skip_workdirs("scan_cap", 1);
                    break;
                }
            }
            if budget <= 0 {
                break;
            }
        }
        ordered.sort_by(|a, b| a.0.cmp(&b.0));
        let mut deleted_bytes = 0i64;
        for (name, path) in ordered {
            if Instant::now() >= deadline {
                report.caps.deadline = true;
                report.skip_workdirs("scan_deadline", 1);
                break;
            }
            report.workdirs.scanned_items += 1;
            let name = name.to_string_lossy();
            let info = match std::fs::symlink_metadata(&path) {
                Ok(info) => info,
                Err(_) => {
                    report.skip_workdirs("stat_failed", 1);
                    continue;
                }
            };
            if !info.is_dir() || info.file_type().is_symlink() {
                report.skip_workdirs("not_workdir", 1);
                continue;
            }
            // A tree this account created, on the volume the policy's
            // watermarks are measured against. Anything else is either not
            // ours to delete or would not move the number that matters.
            if info.uid() != euid() || info.dev() != home_device {
                report.skip_workdirs("unsafe_owner_or_device", 1);
                continue;
            }
            match job_id(&name) {
                // A job workdir is judged by its job's state, never its age:
                // the job that owns it is either coming back to it or it is
                // terminal, and no amount of elapsed time changes which.
                Some(id) => {
                    if live_jobs.iter().any(|live| live == id) {
                        report.skip_workdirs("job_queued_or_running", 1);
                        continue;
                    }
                }
                // Bootstrap scratch has no job behind it, so the policy's
                // retention window is all there is to wait for.
                None => {
                    if info.mtime() as f64 > now - configured.min_age_seconds as f64 {
                        report.skip_workdirs("too_young", 1);
                        continue;
                    }
                }
            }
            report.workdirs.eligible_items += 1;
            let expected = dir_size(&path);
            report.workdirs.expected_bytes += expected;
            if policy.mode != "enforce" {
                continue;
            }
            if report.workdirs.deleted_items >= policy.max_items_per_pass {
                report.caps.items = true;
                report.skip_workdirs("item_cap", 1);
                continue;
            }
            if deleted_bytes >= policy.max_bytes_per_pass {
                report.caps.bytes = true;
                report.skip_workdirs("byte_cap", 1);
                continue;
            }
            if free_bytes(home)? >= policy.target_free_gb * GIB {
                break;
            }
            // The workdir must still be a direct child of a root it was
            // enumerated from, the same lexical check the weles and clone scans
            // make before they remove a tree.
            if !path
                .parent()
                .is_some_and(|parent| scan_roots.iter().any(|root| parent == root.as_path()))
            {
                report.skip_workdirs("escapes_root", 1);
                continue;
            }
            let delete_attempt = (|| -> Result<i64, JanitorError> {
                let before = free_bytes(home)?;
                remove_tree(&path)?;
                Ok(free_bytes(home)? - before)
            })();
            match delete_attempt {
                Ok(delta) => {
                    report.workdirs.actual_free_delta_bytes += delta.max(0);
                    report.workdirs.deleted_items += 1;
                    deleted_bytes += expected;
                }
                Err(exc) => report.add_error(CLEANER, &exc),
            }
        }
        Ok(())
    };
    if let Err(exc) = body(report) {
        report.add_error(CLEANER, &exc);
    }
}
