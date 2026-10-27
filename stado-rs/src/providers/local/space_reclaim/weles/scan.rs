//! The ordered series of refusals, and the eviction that follows whichever
//! runs survive all of them.

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use super::super::{euid, free_bytes, CleanupReport, JanitorError};
use super::tree_ops::{dir_size, remove_tree};

/// The name this cleaner's report is filed under.
pub const CLEANER: &str = "weles_recordings";

/// Where this host's Weles worker writes recordings: the registry's
/// `targets[].weles.recordings_dir`, the one place Stado's `weles-recordings`
/// workload also writes into the Weles LaunchAgents as `WELES_RECORDINGS_ROOT`.
/// A host that declares none has no root this cleaner may sweep; no fixed
/// home path stands in for it.
pub enum RecordingsRoot {
    /// The registry declares no `weles.recordings_dir` for this host.
    Undeclared,
    /// The declared directory does not exist (yet).
    Absent,
    /// The declared directory.
    Present(PathBuf),
}

pub fn recordings_root(declared: Option<&str>) -> RecordingsRoot {
    let Some(declared) = declared else {
        return RecordingsRoot::Undeclared;
    };
    let expanded = crate::config_file::expand_tilde(declared);
    if expanded.is_dir() {
        RecordingsRoot::Present(expanded)
    } else {
        RecordingsRoot::Absent
    }
}

/// Scan the Weles recordings root and evict every run directory in it.
pub fn scan_weles(
    home: &Path,
    declared_root: Option<&str>,
    enforcing: bool,
    report: &mut CleanupReport,
) {
    // Every skipped entry is recorded under its reason, and the report counts
    // them per reason once the pass is over, however it ended.
    let mut skipped: Vec<&'static str> = Vec::new();
    let outcome = walk(home, declared_root, enforcing, report, &mut skipped);
    for reason in skipped.iter().copied().collect::<BTreeSet<_>>() {
        let count = skipped.iter().filter(|seen| **seen == reason).count();
        report.skip_weles(reason, count as i64);
    }
    if let Err(exc) = outcome {
        report.add_error(CLEANER, &exc);
    }
}

fn walk(
    home: &Path,
    declared_root: Option<&str>,
    enforcing: bool,
    report: &mut CleanupReport,
    skipped: &mut Vec<&'static str>,
) -> Result<(), JanitorError> {
    let root = match recordings_root(declared_root) {
        RecordingsRoot::Undeclared => {
            // Named for what is missing and who supplies it: the host's
            // registry entry, written by the `weles-recordings` workload.
            skipped.push("recordings_dir_undeclared");
            return Ok(());
        }
        RecordingsRoot::Absent => {
            skipped.push("root_absent");
            return Ok(());
        }
        RecordingsRoot::Present(root) => root,
    };
    let mut ordered: Vec<(OsString, PathBuf)> = Vec::new();
    for entry in std::fs::read_dir(&root)? {
        let entry = entry?;
        ordered.push((entry.file_name(), entry.path()));
    }
    ordered.sort_by(|a, b| a.0.cmp(&b.0));
    let home_device = std::fs::metadata(home)?.dev();
    report.weles.scanned_items += ordered.len() as i64;
    let mut eligible = Vec::new();
    let mut deleted = Vec::new();
    for (name, path) in ordered {
        if name == "local" || name.to_string_lossy().starts_with('.') {
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
        if info.uid() != euid() || info.dev() != home_device {
            skipped.push("unsafe_owner_or_device");
            continue;
        }
        report.weles.expected_bytes += dir_size(&path);
        eligible.push(path.clone());
        if !enforcing {
            continue;
        }
        // The run directory must stay a direct child of the root.
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
                    report.weles.actual_free_delta_bytes += delta;
                }
                deleted.push(path);
            }
            Err(exc) => report.add_error(CLEANER, &exc),
        }
    }
    report.weles.eligible_items += eligible.len() as i64;
    report.weles.deleted_items += deleted.len() as i64;
    Ok(())
}
