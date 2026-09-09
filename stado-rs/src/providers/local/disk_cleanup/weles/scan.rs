//! The ordered series of refusals, and the bounded eviction that follows
//! whichever runs survive all of them.

use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::Instant;

use super::super::{euid, fixed_root, free_bytes, CleanupReport, JanitorError, GIB};
use super::eligibility::{run_active, upload_proof_ok};
use super::tree_ops::{dir_size, remove_tree};
use crate::targets::DiskCleanupPolicy;

/// Scan the weles recordings root and evict eligible runs.
/// Python `_scan_weles`.
pub fn scan_weles(
    home: &Path,
    policy: &DiskCleanupPolicy,
    now: f64,
    remaining_scan: i64,
    deadline: Instant,
    report: &mut CleanupReport,
) {
    let Some(configured) = policy.cleaners.get("weles_recordings") else {
        return;
    };
    if remaining_scan <= 0 {
        return;
    }
    let body = |report: &mut CleanupReport| -> Result<(), JanitorError> {
        let root = if let Some(configured_root) = &configured.root {
            let expanded = crate::config_file::expand_tilde(configured_root);
            if !expanded.is_dir() {
                report.skip_weles("root_absent", 1);
                return Ok(());
            }
            expanded
        } else {
            let parts = [
                std::ffi::OsString::from("weles"),
                std::ffi::OsString::from("recordings"),
            ];
            match fixed_root(home, &parts, false)? {
                Some(root) => root,
                None => {
                    report.skip_weles("root_absent", 1);
                    return Ok(());
                }
            }
        };
        let mut ordered: Vec<(std::ffi::OsString, PathBuf)> = Vec::new();
        {
            let entries = std::fs::read_dir(&root)?;
            for entry in entries {
                if Instant::now() >= deadline {
                    report.caps.deadline = true;
                    report.skip_weles("scan_deadline", 1);
                    break;
                }
                let entry = entry?;
                ordered.push((entry.file_name(), entry.path()));
                if ordered.len() as i64 >= remaining_scan {
                    break;
                }
            }
        }
        ordered.sort_by(|a, b| a.0.cmp(&b.0));
        let home_device = std::fs::metadata(home)?.dev();
        let mut deleted_bytes = 0i64;
        for (name, path) in ordered {
            if Instant::now() >= deadline {
                report.caps.deadline = true;
                report.skip_weles("scan_deadline", 1);
                break;
            }
            report.weles.scanned_items += 1;
            if name == "local" || name.to_string_lossy().starts_with('.') {
                report.skip_weles("reserved_or_hidden", 1);
                continue;
            }
            let info = match std::fs::symlink_metadata(&path) {
                Ok(info) => info,
                Err(_) => {
                    report.skip_weles("stat_failed", 1);
                    continue;
                }
            };
            if !info.is_dir() || info.file_type().is_symlink() {
                report.skip_weles("not_run_directory", 1);
                continue;
            }
            if info.uid() != euid() || info.dev() != home_device {
                report.skip_weles("unsafe_owner_or_device", 1);
                continue;
            }
            if info.mtime() as f64 > now - configured.min_age_seconds as f64 {
                report.skip_weles("too_young", 1);
                continue;
            }
            if !configured.allow_missing_upload_proof && !upload_proof_ok(&path) {
                // Without durable whole-run upload proof, age alone is never
                // sufficient authorization to delete (default, conservative).
                report.skip_weles("upload_proof_unavailable_v1", 1);
                continue;
            }
            if run_active(&path, now - configured.min_age_seconds as f64) {
                report.skip_weles("active_run", 1);
                continue;
            }
            report.weles.eligible_items += 1;
            let expected = dir_size(&path);
            report.weles.expected_bytes += expected;
            if policy.mode != "enforce" {
                continue;
            }
            if report.weles.deleted_items >= policy.max_items_per_pass {
                report.caps.items = true;
                report.skip_weles("item_cap", 1);
                continue;
            }
            if deleted_bytes >= policy.max_bytes_per_pass {
                report.caps.bytes = true;
                report.skip_weles("byte_cap", 1);
                continue;
            }
            if free_bytes(home)? >= policy.target_free_gb * GIB {
                break;
            }
            // os.path.commonpath([root, entry.path]) != root — lexical
            // escape check; the run dir must stay a direct child.
            if path.parent() != Some(root.as_path()) {
                report.skip_weles("escapes_root", 1);
                continue;
            }
            // Python wraps the free-space probes + rmtree in one
            // try/except (OSError, shutil.Error) per entry.
            let delete_attempt = (|| -> Result<i64, JanitorError> {
                let before = free_bytes(home)?;
                remove_tree(&path)?;
                Ok(free_bytes(home)? - before)
            })();
            match delete_attempt {
                Ok(delta) => {
                    report.weles.actual_free_delta_bytes += delta.max(0);
                    report.weles.deleted_items += 1;
                    deleted_bytes += expected;
                }
                Err(exc) => report.add_error("weles_recordings", &exc),
            }
        }
        Ok(())
    };
    if let Err(exc) = body(report) {
        report.add_error("weles_recordings", &exc);
    }
}
