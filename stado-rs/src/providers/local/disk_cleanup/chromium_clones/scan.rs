//! The pass: enumerate the clone root, apply the ordered refusals, and evict
//! the clones of launches that are over.

use std::ffi::OsString;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use super::super::weles::{dir_size, remove_tree};
use super::super::{euid, free_bytes, CleanupReport, JanitorError};
use super::names::{CLEANER, CLONE_ENTRY_PREFIX};
use super::processes::{held, process_snapshot};
use super::root::default_root;

/// Scan the Chromium clone root and evict the clones of finished launches.
pub fn scan_chromium_clones(home: &Path, enforcing: bool, report: &mut CleanupReport) {
    let body = |report: &mut CleanupReport| -> Result<(), JanitorError> {
        let Some(root) = default_root() else {
            report.skip_clones("root_absent", 1);
            return Ok(());
        };
        if !root.is_dir() {
            // A host that has never launched Chromium, or a host that is not a
            // Mac. Neither is a fault, and neither is a reason to look
            // anywhere else.
            report.skip_clones("root_absent", 1);
            return Ok(());
        }
        let mut ordered: Vec<(OsString, PathBuf)> = Vec::new();
        for entry in std::fs::read_dir(&root)? {
            let entry = entry?;
            ordered.push((entry.file_name(), entry.path()));
        }
        ordered.sort_by(|a, b| a.0.cmp(&b.0));
        let home_device = std::fs::metadata(home)?.dev();
        // The most recent CLONE of the enumerated set, by the mtime macOS
        // stamped when it made it. Kept whatever else is true: a browser
        // session still running owns exactly this one, and nothing in the OS
        // says which session that is.
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
        for (name, path) in ordered {
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
            // A clone the OS made for THIS account, on the volume the rule
            // measures. Anything else is either not ours to delete or would
            // not move the number that matters.
            if info.uid() != euid() || info.dev() != home_device {
                report.skip_clones("unsafe_owner_or_device", 1);
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
            if !enforcing {
                continue;
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
