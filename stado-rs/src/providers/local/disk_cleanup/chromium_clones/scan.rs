//! The pass: enumerate the clone root, apply the ordered refusals, and evict
//! the clones of launches that are over.

use std::ffi::OsString;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::Instant;

use super::super::weles::{dir_size, remove_tree};
use super::super::{euid, free_bytes, CleanupReport, JanitorError, GIB};
use super::names::{CLEANER, CLONE_ENTRY_PREFIX};
use super::processes::{held, process_snapshot};
use super::root::default_root;
use crate::targets::DiskCleanupPolicy;

/// Scan the Chromium clone root and evict the clones of finished launches.
///
/// `remaining_scan` is this cleaner's share of `max_scan_items` left by the
/// cleaners that ran before it, and `deadline` is the pass deadline the HF and
/// build-cache scans honour. It matters here and not in
/// [`super::super::weles`]: sizing one clone means walking a whole browser
/// bundle, and the root holds one per launch.
pub fn scan_chromium_clones(
    home: &Path,
    policy: &DiskCleanupPolicy,
    now: f64,
    remaining_scan: i64,
    deadline: Instant,
    report: &mut CleanupReport,
) {
    let Some(configured) = policy.cleaners.get(CLEANER) else {
        return;
    };
    if remaining_scan <= 0 {
        return;
    }
    let body = |report: &mut CleanupReport| -> Result<(), JanitorError> {
        let root = match &configured.root {
            Some(configured_root) => crate::config_file::expand_tilde(configured_root),
            None => match default_root() {
                Some(root) => root,
                None => {
                    report.skip_clones("root_absent", 1);
                    return Ok(());
                }
            },
        };
        if !root.is_dir() {
            // A host that has never launched Chromium, or a host that is not a
            // Mac. Neither is a fault, and neither is a reason to look
            // anywhere else.
            report.skip_clones("root_absent", 1);
            return Ok(());
        }
        let mut ordered: Vec<(OsString, PathBuf)> = Vec::new();
        {
            let entries = std::fs::read_dir(&root)?;
            for entry in entries {
                let entry = entry?;
                ordered.push((entry.file_name(), entry.path()));
                if ordered.len() as i64 >= remaining_scan {
                    report.caps.scan = true;
                    report.skip_clones("scan_cap", 1);
                    break;
                }
            }
        }
        ordered.sort_by(|a, b| a.0.cmp(&b.0));
        let home_device = std::fs::metadata(home)?.dev();
        // The most recent CLONE of the enumerated set, by the mtime macOS
        // stamped when it made it. Kept whatever else is true — see the module
        // header: a session older than the retention window still owns exactly
        // this one, and nothing in the OS says which session that is.
        //
        // Chosen among entries that could be candidates at all, and not among
        // everything in the root: a stray directory nobody launched, sitting
        // there with the freshest mtime, would otherwise "protect" itself and
        // leave the live browser's clone as the newest thing eligible — the
        // guard inverted into the one deletion it exists to prevent.
        let newest = ordered
            .iter()
            .filter(|(name, _)| name.to_string_lossy().starts_with(CLONE_ENTRY_PREFIX))
            .filter_map(|(_, path)| {
                let info = std::fs::symlink_metadata(path).ok()?;
                let clone = info.is_dir() && !info.file_type().is_symlink();
                clone.then(|| (info.mtime(), path.clone()))
            })
            .max_by_key(|(mtime, _)| *mtime)
            .map(|(_, path)| path);
        // Without a process table there is no live-process gate, and this
        // cleaner does not delete with a gate missing.
        let Some(snapshot) = process_snapshot() else {
            report.skip_clones("process_table_unavailable", ordered.len() as i64);
            return Ok(());
        };
        let mut deleted_bytes = 0i64;
        for (name, path) in ordered {
            if Instant::now() >= deadline {
                report.caps.deadline = true;
                report.skip_clones("scan_deadline", 1);
                break;
            }
            report.clones.scanned_items += 1;
            let name = name.to_string_lossy();
            if !name.starts_with(CLONE_ENTRY_PREFIX) {
                report.skip_clones("reserved_or_hidden", 1);
                continue;
            }
            let info = match std::fs::symlink_metadata(&path) {
                Ok(info) => info,
                Err(_) => {
                    report.skip_clones("stat_failed", 1);
                    continue;
                }
            };
            if !info.is_dir() || info.file_type().is_symlink() {
                report.skip_clones("not_run_directory", 1);
                continue;
            }
            // A clone the OS made for THIS account, on the volume the policy's
            // watermarks are measured against. Anything else is either not
            // ours to delete or would not move the number that matters.
            if info.uid() != euid() || info.dev() != home_device {
                report.skip_clones("unsafe_owner_or_device", 1);
                continue;
            }
            if info.mtime() as f64 > now - configured.min_age_seconds as f64 {
                report.skip_clones("too_young", 1);
                continue;
            }
            if newest.as_deref() == Some(path.as_path()) {
                report.skip_clones("newest_clone", 1);
                continue;
            }
            if held(&snapshot, &path) {
                report.skip_clones("active_run", 1);
                continue;
            }
            report.clones.eligible_items += 1;
            let expected = dir_size(&path);
            report.clones.expected_bytes += expected;
            if policy.mode != "enforce" {
                continue;
            }
            if report.clones.deleted_items >= policy.max_items_per_pass {
                report.caps.items = true;
                report.skip_clones("item_cap", 1);
                continue;
            }
            if deleted_bytes >= policy.max_bytes_per_pass {
                report.caps.bytes = true;
                report.skip_clones("byte_cap", 1);
                continue;
            }
            if free_bytes(home)? >= policy.target_free_gb * GIB {
                break;
            }
            // The clone must still be a direct child of the root it was
            // enumerated from, the same lexical check the weles scan makes
            // before it removes a run.
            if path.parent() != Some(root.as_path()) {
                report.skip_clones("escapes_root", 1);
                continue;
            }
            let delete_attempt = (|| -> Result<i64, JanitorError> {
                let before = free_bytes(home)?;
                remove_tree(&path)?;
                Ok(free_bytes(home)? - before)
            })();
            match delete_attempt {
                Ok(delta) => {
                    report.clones.actual_free_delta_bytes += delta.max(0);
                    report.clones.deleted_items += 1;
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
