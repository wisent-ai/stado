//! The pass: enumerate the clone root, apply the ordered refusals, and evict
//! the clones of launches that are over.

use std::collections::BTreeSet;
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
    // Every skipped entry is recorded under its reason, and the report counts
    // them per reason once the pass is over, however it ended.
    let mut skipped: Vec<&'static str> = Vec::new();
    let outcome = walk(home, enforcing, report, &mut skipped);
    for reason in skipped.iter().copied().collect::<BTreeSet<_>>() {
        let count = skipped.iter().filter(|seen| **seen == reason).count();
        report.skip_clones(reason, count as i64);
    }
    if let Err(exc) = outcome {
        report.add_error(CLEANER, &exc);
    }
}

fn walk(
    home: &Path,
    enforcing: bool,
    report: &mut CleanupReport,
    skipped: &mut Vec<&'static str>,
) -> Result<(), JanitorError> {
    let Some(root) = default_root() else {
        skipped.push("root_absent");
        return Ok(());
    };
    if !root.is_dir() {
        // A host that has never launched Chromium, or a host that is not a
        // Mac. Neither is a fault, and neither is a reason to look anywhere
        // else.
        skipped.push("root_absent");
        return Ok(());
    }
    let mut ordered: Vec<(OsString, PathBuf)> = Vec::new();
    for entry in std::fs::read_dir(&root)? {
        let entry = entry?;
        ordered.push((entry.file_name(), entry.path()));
    }
    ordered.sort_by(|a, b| a.0.cmp(&b.0));
    let home_device = std::fs::metadata(home)?.dev();
    // The most recent CLONE of the enumerated set, by the mtime macOS stamped
    // when it made it. Kept whatever else is true: a browser session still
    // running owns exactly this one, and nothing in the OS says which session
    // that is.
    //
    // Chosen among entries that could be candidates at all, and not among
    // everything in the root: a stray directory nobody launched, sitting there
    // with the freshest mtime, would otherwise "protect" itself and leave the
    // live browser's clone as the newest thing eligible — the guard inverted
    // into the one deletion it exists to prevent. For the same reason a clone
    // that cannot be read while choosing leaves the newest one unknown, and
    // then nothing is deleted: leaving it out would protect an older clone
    // instead of the one the browser may be using.
    let mut clones = Vec::new();
    let mut newest_unknown = false;
    for (name, path) in &ordered {
        if !name.to_string_lossy().starts_with(CLONE_ENTRY_PREFIX) {
            continue;
        }
        match std::fs::symlink_metadata(path) {
            Ok(info) if info.is_dir() && !info.file_type().is_symlink() => {
                clones.push((info.mtime(), path.clone()));
            }
            Ok(_not_a_clone) => {}
            Err(_unreadable) => newest_unknown = true,
        }
    }
    if newest_unknown {
        skipped.extend(std::iter::repeat_n("newest_unknown", ordered.len()));
        return Ok(());
    }
    let newest = clones
        .into_iter()
        .max_by_key(|(mtime, _)| *mtime)
        .map(|(_, path)| path);
    // Without a process table there is no live-process gate, and this cleaner
    // does not delete with a gate missing.
    let Some(snapshot) = process_snapshot() else {
        skipped.extend(std::iter::repeat_n("process_table_unavailable", ordered.len()));
        return Ok(());
    };
    report.clones.scanned_items += ordered.len() as i64;
    let mut eligible = Vec::new();
    let mut deleted = Vec::new();
    for (name, path) in ordered {
        let name = name.to_string_lossy();
        if !name.starts_with(CLONE_ENTRY_PREFIX) {
            skipped.push("reserved_or_hidden");
            continue;
        }
        let Ok(info) = std::fs::symlink_metadata(&path) else {
            skipped.push("stat_failed");
            continue;
        };
        if !info.is_dir() || info.file_type().is_symlink() {
            skipped.push("not_run_directory");
            continue;
        }
        // A clone the OS made for THIS account, on the volume the rule
        // measures. Anything else is either not ours to delete or would not
        // move the number that matters.
        if info.uid() != euid() || info.dev() != home_device {
            skipped.push("unsafe_owner_or_device");
            continue;
        }
        if newest.as_deref() == Some(path.as_path()) {
            skipped.push("newest_clone");
            continue;
        }
        if held(&snapshot, &path) {
            skipped.push("active_run");
            continue;
        }
        report.clones.expected_bytes += dir_size(&path);
        eligible.push(path.clone());
        if !enforcing {
            continue;
        }
        // The clone must still be a direct child of the root it was
        // enumerated from, the same lexical check the weles scan makes before
        // it removes a run.
        if path.parent() != Some(root.as_path()) {
            skipped.push("escapes_root");
            continue;
        }
        let delete_attempt = (|| -> Result<i64, JanitorError> {
            let before = free_bytes(home)?;
            remove_tree(&path)?;
            Ok(free_bytes(home)? - before)
        })();
        match delete_attempt {
            Ok(delta) => {
                // Other writers can shrink the free space meanwhile; only a
                // gain is credited to this deletion.
                if delta.is_positive() {
                    report.clones.actual_free_delta_bytes += delta;
                }
                deleted.push(path);
            }
            Err(exc) => report.add_error(CLEANER, &exc),
        }
    }
    report.clones.eligible_items += eligible.len() as i64;
    report.clones.deleted_items += deleted.len() as i64;
    Ok(())
}
