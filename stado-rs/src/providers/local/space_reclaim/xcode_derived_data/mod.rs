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
use std::process::Command;

use super::candidates::ledger;
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

/// The open-file tables, in the places the operating systems put them.
const LSOF: &[&str] = &["/usr/sbin/lsof", "/usr/bin/lsof"];

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

/// One command's standard output, or why it could not be read, on stderr.
fn read(program: &str, args: &[&str]) -> Option<String> {
    match crate::wait::output(Command::new(program).args(args)) {
        // `lsof` exits 1 when one of the processes it lists vanished while
        // it read; what it printed for the rest is still the table.
        Ok(output) if output.status.success() || !output.stdout.is_empty() => {
            Some(String::from_utf8_lossy(&output.stdout).into_owned())
        }
        Ok(output) => {
            eprintln!(
                "xcode derived data: {program} answered {} ({}); no project directory is \
                 removed this pass",
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            );
            None
        }
        Err(error) => {
            eprintln!(
                "xcode derived data: {program} could not be run ({error}); no project directory \
                 is removed this pass"
            );
            None
        }
    }
}

/// Every path a live process has open or works in, and every live process's
/// arguments, each read once for the pass; `None` when either cannot be
/// read.
fn holders() -> Option<(Vec<String>, String)> {
    let lsof = LSOF
        .iter()
        .find(|program| Path::new(program).is_file())
        .or_else(|| {
            eprintln!(
                "xcode derived data: no lsof at {}; no project directory is removed this pass",
                LSOF.join(" or ")
            );
            None
        })?;
    let open = read(lsof, &["-n", "-P", "-w", "-F", "n"])?
        .lines()
        .filter_map(|line| line.strip_prefix('n'))
        .map(str::to_string)
        .collect();
    Some((open, read("/bin/ps", &["-Ao", "args="])?))
}

/// One pass over the project directories: each measured, and in an enforcing
/// pass removed unless a build tool runs, a live process names it, or it lies
/// outside the tree this host lets the janitor delete in.
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
    for entry in entries.flatten() {
        let is_directory = entry
            .file_type()
            .is_ok_and(|kind| kind.is_dir() && !kind.is_symlink());
        if is_directory && entry.file_name().to_str().is_some_and(project_directory) {
            projects.push(entry.path());
        }
    }
    projects.sort();
    met.listed(projects.len());
    let table = if enforcing { holders() } else { None };
    for path in projects {
        let size = u64::try_from(dir_size(&path)).unwrap_or_default();
        met.eligible(size);
        if !enforcing {
            continue;
        }
        let Some((open, argv)) = &table else {
            met.kept("process_table_unread", size);
            continue;
        };
        let held = open.iter().any(|name| Path::new(name).starts_with(&path));
        if held || argv.contains(path.to_string_lossy().as_ref()) {
            met.kept("held_by_process", size);
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
        match remove_tree(&path) {
            Ok(()) => {
                met.removed(Some(size));
                let bytes = i64::try_from(size).unwrap_or(i64::MAX);
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
