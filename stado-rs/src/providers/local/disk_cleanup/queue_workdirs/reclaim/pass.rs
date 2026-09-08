//! One pass of the queue-workdir cleaner: the keep-list gate, the canonical
//! root's ordered walk, and the report it writes.

use std::ffi::OsString;
use std::io;
use std::os::fd::AsRawFd;
use std::path::Path;
use std::time::Instant;

use crate::providers::local::disk_cleanup::queue_workdirs::roots::{job_id, open_work_root_in};
use crate::providers::local::disk_cleanup::queue_workdirs::{CLEANER, WORKDIR_PREFIX};
use crate::providers::local::disk_cleanup::weles::dir_size;
use crate::providers::local::disk_cleanup::{
    euid, free_bytes, ifmt, safefs, CleanupReport, JanitorError, GIB, IFDIR,
};
use crate::targets::DiskCleanupPolicy;

use super::legacy::reclaim_legacy_bridges;
use super::{remove_tree_at, same_object};

/// Evict the workdirs of terminal jobs, oldest scan order first.
///
/// `live_jobs` is the keep-list: every job id currently in `queue` or `running`.
/// `None` means the queue store could not be read this pass, and this cleaner
/// then removes nothing at all.
pub fn scan_queue_workdirs(
    home: &Path,
    policy: &DiskCleanupPolicy,
    _now: f64,
    remaining_scan: i64,
    deadline: Instant,
    live_jobs: Option<&[String]>,
    report: &mut CleanupReport,
) {
    let Some(_configured) = policy.cleaners.get(CLEANER) else {
        return;
    };
    if remaining_scan <= 0 {
        return;
    }
    let body = |report: &mut CleanupReport| -> Result<(), JanitorError> {
        // Without the keep-list there is no terminal-job gate, so this cleaner
        // removes nothing rather than risk a live job's persistent tree.
        let Some(live_jobs) = live_jobs else {
            report.skip_workdirs("queue_store_unreadable", 1);
            return Ok(());
        };
        // Admission and cleanup traverse the same three owned components from
        // the physically resolved home. Every component is O_DIRECTORY |
        // O_NOFOLLOW, so replacing `.stado`, `work`, or `jobs` with a symlink
        // cannot redirect this pass.
        let (canonical_root, root_fd, home_device) = match open_work_root_in(home, false) {
            Ok(opened) => opened,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                report.skip_workdirs("root_absent", 1);
                return Ok(());
            }
            Err(_) => {
                report.skip_workdirs("unsafe_root", 1);
                return Ok(());
            }
        };
        let root_info = safefs::fstat(root_fd.as_raw_fd())?;
        if root_info.st_dev != home_device {
            report.skip_workdirs("unsafe_root", 1);
            return Ok(());
        }
        // Reserve at most one eighth (and never more than 16 entries) for old
        // agent links. Canonical work stays dominant while a full canonical
        // root cannot consume the legacy pass's whole share.
        let legacy_budget = (remaining_scan / 8)
            .clamp(1, 16)
            .min(remaining_scan.saturating_sub(1));
        let mut ordered: Vec<OsString> = Vec::new();
        let mut budget = remaining_scan - legacy_budget;
        for name in safefs::DirEntries::open(root_fd.as_raw_fd())? {
            let name = name?;
            if !name.to_string_lossy().starts_with(WORKDIR_PREFIX) {
                continue;
            }
            ordered.push(name);
            budget -= 1;
            if budget <= 0 {
                report.caps.scan = true;
                report.skip_workdirs("scan_cap", 1);
                break;
            }
        }
        ordered.sort();
        let mut deleted_bytes = 0i64;
        for name in ordered {
            if Instant::now() >= deadline {
                report.caps.deadline = true;
                report.skip_workdirs("scan_deadline", 1);
                break;
            }
            report.workdirs.scanned_items += 1;
            let info = match safefs::fstatat_nofollow(root_fd.as_raw_fd(), &name) {
                Ok(info) => info,
                Err(_) => {
                    report.skip_workdirs("stat_failed", 1);
                    continue;
                }
            };
            if ifmt(info.st_mode as u32) != IFDIR {
                report.skip_workdirs("not_workdir", 1);
                continue;
            }
            if info.st_uid != euid() || info.st_dev != root_info.st_dev {
                report.skip_workdirs("unsafe_owner_or_device", 1);
                continue;
            }
            let work_fd = match safefs::open_dir_at(root_fd.as_raw_fd(), &name) {
                Ok(work_fd) => work_fd,
                Err(_) => {
                    report.skip_workdirs("entry_replaced", 1);
                    continue;
                }
            };
            if !same_object(&safefs::fstat(work_fd.as_raw_fd())?, &info) {
                report.skip_workdirs("entry_replaced", 1);
                continue;
            }
            let name_text = name.to_string_lossy();
            let Some(id) = job_id(&name_text) else {
                report.skip_workdirs("not_workdir", 1);
                continue;
            };
            if live_jobs.iter().any(|live| live == id) {
                report.skip_workdirs("job_queued_or_running", 1);
                continue;
            }
            report.workdirs.eligible_items += 1;
            let expected = dir_size(&canonical_root.join(&name));
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
            let delete_attempt = (|| -> Result<i64, JanitorError> {
                let before = free_bytes(home)?;
                remove_tree_at(
                    root_fd.as_raw_fd(),
                    &name,
                    work_fd.as_raw_fd(),
                    root_info.st_dev,
                )?;
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
        reclaim_legacy_bridges(
            home,
            policy,
            live_jobs,
            &canonical_root,
            legacy_budget,
            deleted_bytes,
            report,
        )?;
        Ok(())
    };
    if let Err(exc) = body(report) {
        report.add_error(CLEANER, &exc);
    }
}
