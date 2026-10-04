//! The ordered series of refusals, and the eviction that follows whichever
//! runs survive all of them.

use std::ffi::OsString;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use super::super::{euid, fixed_root, free_bytes, CleanupReport, JanitorError};
use super::tree_ops::{dir_size, remove_tree};

/// The name this cleaner's report is filed under.
pub const CLEANER: &str = "weles_recordings";

/// Where this host's Weles worker writes recordings: the registry's
/// `targets[].weles.recordings_dir` when the host declares one, otherwise
/// `~/weles/recordings`. `None` when neither exists.
pub fn recordings_root(
    home: &Path,
    declared: Option<&str>,
) -> Result<Option<PathBuf>, JanitorError> {
    if let Some(declared) = declared {
        let expanded = crate::config_file::expand_tilde(declared);
        return Ok(expanded.is_dir().then_some(expanded));
    }
    fixed_root(
        home,
        &[OsString::from("weles"), OsString::from("recordings")],
        false,
    )
}

/// Scan the Weles recordings root and evict every run directory in it.
pub fn scan_weles(
    home: &Path,
    declared_root: Option<&str>,
    enforcing: bool,
    report: &mut CleanupReport,
) {
    let body = |report: &mut CleanupReport| -> Result<(), JanitorError> {
        let Some(root) = recordings_root(home, declared_root)? else {
            report.skip_weles("root_absent", 1);
            return Ok(());
        };
        let mut ordered: Vec<(OsString, PathBuf)> = Vec::new();
        for entry in std::fs::read_dir(&root)? {
            let entry = entry?;
            ordered.push((entry.file_name(), entry.path()));
        }
        ordered.sort_by(|a, b| a.0.cmp(&b.0));
        let home_device = std::fs::metadata(home)?.dev();
        for (name, path) in ordered {
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
            report.weles.eligible_items += 1;
            report.weles.expected_bytes += dir_size(&path);
            if !enforcing {
                continue;
            }
            // The run directory must stay a direct child of the root.
            if path.parent() != Some(root.as_path()) {
                report.skip_weles("escapes_root", 1);
                continue;
            }
            let delete_attempt = (|| -> Result<i64, JanitorError> {
                let before = free_bytes(home)?;
                remove_tree(&path)?;
                Ok(free_bytes(home)? - before)
            })();
            match delete_attempt {
                Ok(delta) => {
                    report.weles.actual_free_delta_bytes += delta.max(0);
                    report.weles.deleted_items += 1;
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
