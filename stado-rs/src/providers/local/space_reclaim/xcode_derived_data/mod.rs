//! Xcode's per-project build directories, taken by the janitor's own pass at
//! the disk-full rule.
//!
//! Every Xcode and `xcodebuild` build of a project writes its intermediates,
//! products and index under `~/Library/Developer/Xcode/DerivedData/
//! <project>-<hash>`, one directory per workspace, and never removes them. A
//! host building the fleet's desktop apps held tens of GiB there that the
//! next build makes again. No tool tags them, and `~/Library` is a folder no
//! walk of the home opens, so no cleaner reached them.
//!
//! Only the directories directly under `DerivedData` whose names end in
//! Xcode's 28-letter workspace hash are taken, each whole, under the
//! janitor's exclusive cleanup lock for that one directory. A directory a
//! running process holds is kept for a later pass, as a build directory a
//! `cargo build` holds is: a build has a file in it open (`lsof`, which
//! lists every open file and working directory of every process) or names it
//! in its arguments (`held_by_process`). When either table cannot be read,
//! every directory is kept (`process_table_unread`). `ModuleCache.noindex`
//! and everything outside `DerivedData` — archives, device support,
//! simulators — are never touched. Each removal is a line of the janitor's
//! removal record.

use std::path::{Path, PathBuf};

use super::candidates::ledger;
use super::holders::Holders;
use super::janitor::pass::lock::file::LockState;
use super::janitor::pass::lock::ownership::{acquire_lock_state, retired_locks_active};
use super::janitor::policy::ownership;
use super::janitor::state::report::{met::Met, CleanupReport};
use super::weles::{dir_size, remove_tree};
use super::JanitorError;

/// The name this cleaner's report is filed under.
pub const CLEANER: &str = "xcode_derived_data";

/// Home-relative directory Xcode keeps its per-project build output in.
pub const ROOT: &str = "Library/Developer/Xcode/DerivedData";

/// The length of the hash Xcode appends to a project's directory name.
const HASH_LETTERS: usize = 28;

/// Whether `name` is a project's build directory: `<project>-<hash>`, the
/// hash 28 lowercase letters.
pub(in crate::providers::local::space_reclaim) fn project_directory(name: &str) -> bool {
    name.rsplit_once('-').is_some_and(|(project, hash)| {
        !project.is_empty()
            && hash.len() == HASH_LETTERS
            && hash.bytes().all(|byte| byte.is_ascii_lowercase())
    })
}

/// One pass over the project directories: each measured, and in an enforcing
/// pass removed unless a live process holds it or it lies outside the tree
/// this host lets the janitor delete in. What holds a directory is read
/// again for each one after the cleanup lock is taken, so a build started
/// while an earlier directory was measured or removed is seen.
pub(in crate::providers::local::space_reclaim) fn scan_xcode_derived_data(
    home: &Path,
    enforcing: bool,
    deletable: Option<&Path>,
    lock_dir: &Path,
    writer: &str,
    report: &mut CleanupReport,
) {
    let mut met = Met::default();
    let root = home.join(ROOT);
    let entries = match std::fs::read_dir(&root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            met.skipped("root_absent");
            finish(met, report);
            return;
        }
        Err(error) => {
            report.add_error(
                CLEANER,
                &JanitorError::os(&format!(
                    "Xcode's build directory {} cannot be listed: {error}",
                    root.display()
                )),
            );
            return;
        }
    };
    let mut projects: Vec<PathBuf> = Vec::new();
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                report.add_error(
                    CLEANER,
                    &JanitorError::os(&format!(
                        "an entry of Xcode's build directory {} cannot be read: {error}",
                        root.display()
                    )),
                );
                continue;
            }
        };
        let is_directory = entry
            .file_type()
            .is_ok_and(|kind| kind.is_dir() && !kind.is_symlink());
        if is_directory && entry.file_name().to_str().is_some_and(project_directory) {
            projects.push(entry.path());
        }
    }
    projects.sort();
    met.listed(projects.len());
    for path in projects {
        // `dir_size` sums lengths, so it is never negative.
        let bytes = dir_size(&path);
        let size = bytes.unsigned_abs();
        met.eligible(size);
        if !enforcing {
            continue;
        }
        if !ownership::may_delete(deletable, &path) {
            met.kept(ownership::NOT_STADO_OWNED, size);
            continue;
        }
        // The exclusive cleanup lock for this one directory, as every other
        // tree the janitor takes whole: a workload's hold keeps it.
        let lock = match acquire_lock_state(lock_dir, writer) {
            Ok(LockState::Held(lock)) => lock,
            Ok(LockState::Busy { .. }) => {
                met.kept("cleanup_lock_busy", size);
                continue;
            }
            Err(error) => {
                report.add_error(CLEANER, &error);
                met.kept("cleanup_lock_unreadable", size);
                continue;
            }
        };
        match retired_locks_active(lock_dir, &lock.file) {
            Ok(holders) if holders.is_empty() => {}
            Ok(_) => {
                met.kept("lock_predecessor_active", size);
                continue;
            }
            Err(error) => {
                report.add_error(CLEANER, &error);
                met.kept("lock_predecessor_unreadable", size);
                continue;
            }
        }
        // Read under the lock, for this directory alone: a build that
        // started after the pass began is seen before anything is removed.
        let Some(holders) = Holders::read(CLEANER) else {
            met.kept("process_table_unread", size);
            continue;
        };
        if holders.hold(&path) {
            met.kept("held_by_process", size);
            continue;
        }
        match remove_tree(&path) {
            Ok(()) => {
                met.removed(Some(size));
                if let Err(error) =
                    ledger::record_tree(lock_dir, CLEANER, writer, &path, bytes, bytes)
                {
                    report.add_error("removal_record", &error);
                }
            }
            Err(error) => {
                met.skipped("deletion_refused");
                report.add_error(
                    CLEANER,
                    &JanitorError::os(&format!("cannot remove {}: {error}", path.display())),
                );
            }
        }
        drop(lock);
    }
    finish(met, report);
}

fn finish(met: Met, report: &mut CleanupReport) {
    match met.report() {
        Ok(record) => report.xcode_derived_data = record,
        Err(cause) => report.add_error(CLEANER, &JanitorError::os(&cause)),
    }
}
