//! The compatibility root one pass ends with: `/tmp/wc-<job_id>`.
//!
//! Runs on the bounded share the canonical walk reserved for it, under the
//! same keep-list, and removes exactly two shapes an older agent can leave
//! behind: an owner-matched symlink whose target is this account's canonical
//! tree, and the tree itself.

use std::os::fd::AsRawFd;
use std::os::unix::fs::MetadataExt;
use std::path::Path;

use crate::providers::local::disk_cleanup::queue_workdirs::roots::job_id;
use crate::providers::local::disk_cleanup::queue_workdirs::{
    CLEANER, LEGACY_WORK_ROOT, WORKDIR_PREFIX,
};
use crate::providers::local::disk_cleanup::weles::dir_size;
use crate::providers::local::disk_cleanup::{
    euid, free_bytes, safefs, CleanupReport, JanitorError,
};
use crate::targets::DiskCleanupPolicy;

use super::{remove_tree_at, same_object};

/// Spend the reserved legacy share, continuing the canonical pass's budget.
pub(super) fn reclaim_legacy_bridges(
    home: &Path,
    policy: &DiskCleanupPolicy,
    live_jobs: &[String],
    canonical_root: &Path,
    legacy_budget: i64,
    mut deleted_bytes: i64,
    report: &mut CleanupReport,
) -> Result<(), JanitorError> {
    // The release bridge used by pre-persistent agents must keep its old
    // `/tmp/wc-*` path alive through terminal artifact upload. The queue
    // store moves a job out of the live set only after that upload, making
    // the same keep-list a deterministic deletion fence for the symlink.
    // The bounded share reserved above is independent of canonical
    // enumeration: a root full of live jobs must not starve terminal
    // compatibility links forever. The pass never traverses a link or
    // deletes a tree, and total canonical-plus-legacy accounting remains
    // capped at `remaining_scan`.
    let legacy_root = Path::new(LEGACY_WORK_ROOT);
    let mut legacy_remaining = legacy_budget;
    if legacy_root.is_dir() && legacy_remaining > 0 {
        let entries = match std::fs::read_dir(legacy_root) {
            Ok(entries) => entries,
            Err(_) => {
                report.skip_workdirs("legacy_root_unreadable", 1);
                return Ok(());
            }
        };
        let legacy_fd = match legacy_root
            .canonicalize()
            .and_then(|root| safefs::open_dir_path(&root))
        {
            Ok(fd) => fd,
            Err(_) => {
                report.skip_workdirs("legacy_root_unreadable", 1);
                return Ok(());
            }
        };
        let legacy_info = safefs::fstat(legacy_fd.as_raw_fd())?;
        for entry in entries {
            if legacy_remaining <= 0 {
                report.caps.scan = true;
                report.skip_workdirs("scan_cap", 1);
                break;
            }
            let entry = entry?;
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if !name.starts_with(WORKDIR_PREFIX) {
                continue;
            }
            legacy_remaining -= 1;
            report.workdirs.scanned_items += 1;
            let Some(id) = job_id(&name) else {
                report.skip_workdirs("not_workdir", 1);
                continue;
            };
            if live_jobs.iter().any(|live| live == id) {
                report.skip_workdirs("job_queued_or_running", 1);
                continue;
            }
            let path = entry.path();
            let info = match std::fs::symlink_metadata(&path) {
                Ok(info) => info,
                Err(_) => {
                    report.skip_workdirs("stat_failed", 1);
                    continue;
                }
            };
            let expected_target = canonical_root.join(format!("{WORKDIR_PREFIX}{id}"));
            let is_bridge = info.file_type().is_symlink()
                && info.uid() == euid()
                && path.parent() == Some(legacy_root)
                && std::fs::read_link(&path).ok().as_deref() == Some(expected_target.as_path());
            // An agent that predates the persistent root left the tree
            // itself here, not a link to it, and no cleaner could see it:
            // this pass only ever unlinked symlinks, and every other
            // cleaner is rooted in the account's home. On 2026-09-04 ten
            // such trees held 14.2 GB on charless-mac-mini while the host
            // sat at 1.1 GB free, which took its object API, the registry
            // authority and every Skarbiec decryption down together while
            // `space reclaim` measured zero in all eight stages. The gate is
            // the canonical pass's own: this account owns it, it is a
            // directory on the legacy root's device, and its job is
            // terminal by the same keep-list.
            let stale_tree = !info.file_type().is_symlink()
                && info.file_type().is_dir()
                && info.uid() == euid()
                && info.dev() == legacy_info.st_dev as u64
                && path.parent() == Some(legacy_root);
            if !is_bridge && !stale_tree {
                report.skip_workdirs("not_legacy_bridge", 1);
                continue;
            }
            report.workdirs.eligible_items += 1;
            let expected = if stale_tree { dir_size(&path) } else { 0 };
            report.workdirs.expected_bytes += expected;
            if policy.mode != "enforce" {
                continue;
            }
            if report.workdirs.deleted_items >= policy.max_items_per_pass {
                report.caps.items = true;
                report.skip_workdirs("item_cap", 1);
                continue;
            }
            if stale_tree && deleted_bytes >= policy.max_bytes_per_pass {
                report.caps.bytes = true;
                report.skip_workdirs("byte_cap", 1);
                continue;
            }
            let outcome = if stale_tree {
                (|| -> Result<i64, JanitorError> {
                    let entry_name = entry.file_name();
                    let stat = safefs::fstatat_nofollow(legacy_fd.as_raw_fd(), &entry_name)?;
                    let work_fd = safefs::open_dir_at(legacy_fd.as_raw_fd(), &entry_name)?;
                    if !same_object(&safefs::fstat(work_fd.as_raw_fd())?, &stat) {
                        return Err(JanitorError::os(
                            "queue workdir entry replaced while deleting",
                        ));
                    }
                    let before = free_bytes(home)?;
                    remove_tree_at(
                        legacy_fd.as_raw_fd(),
                        &entry_name,
                        work_fd.as_raw_fd(),
                        legacy_info.st_dev,
                    )?;
                    Ok(free_bytes(home)? - before)
                })()
            } else {
                std::fs::remove_file(&path)
                    .map(|()| 0)
                    .map_err(JanitorError::from)
            };
            match outcome {
                Ok(delta) => {
                    report.workdirs.actual_free_delta_bytes += delta.max(0);
                    report.workdirs.deleted_items += 1;
                    deleted_bytes += expected;
                }
                Err(exc) => report.add_error(CLEANER, &exc),
            }
        }
    }
    Ok(())
}
