//! Cargo's per-workspace build directories under its home, taken by the
//! janitor's own pass at the disk-full rule.
//!
//! The janitor declares `build.build-dir = "{cargo-cache-home}/build/
//! {workspace-path-hash}"` for checkouts it cannot walk
//! ([`super::placement`]), so every such checkout's intermediate build
//! output lands in `$CARGO_HOME/build/<xx>/<hash>`. Cargo does not always tag
//! those directories, and the tagged-tree walk stops at a tag, so before this
//! cleaner only `stado space reclaim <host> --stage cargo_build_dirs` took
//! them: a laptop at the threshold kept 49 GiB of them until its owner
//! deleted them by hand. Everything in one is Cargo's own output for one
//! workspace and comes back with that workspace's next build.
//!
//! Only the two fixed levels are listed. Each directory is taken whole
//! through directory descriptors ([`super::remove`]), under the janitor's
//! exclusive cleanup lock for that one directory, and only when no build
//! holds a `.cargo-lock` in it ([`super::held`]): a running `cargo build`
//! keeps its directory. Each removal is a line of the janitor's removal
//! record ([`crate::providers::local::space_reclaim::candidates::ledger`]).

use std::ffi::OsString;
use std::os::fd::{AsRawFd, RawFd};
use std::path::{Path, PathBuf};

use super::{entry_names, held, remove::remove_tree, reserved::cargo_home, same_object};
use crate::providers::local::space_reclaim::candidates::ledger;
use crate::providers::local::space_reclaim::janitor::pass::lock::file::LockState;
use crate::providers::local::space_reclaim::janitor::pass::lock::ownership::{
    acquire_lock_state, retired_locks_active,
};
use crate::providers::local::space_reclaim::janitor::policy::ownership;
use crate::providers::local::space_reclaim::{
    euid, free_bytes, ifmt, safefs, CleanupReport, JanitorError, IFDIR,
};

/// The name this cleaner's report is filed under.
pub const CARGO_BUILD_DIRS: &str = "cargo_build_dirs";

/// `{cargo-cache-home}/build`, where the declared build-dir puts every
/// workspace's build directory.
pub fn cargo_build_root(home: &Path) -> PathBuf {
    cargo_home(home).join("build")
}

/// Every byte allocated under `dir_fd`, never leaving `device`. An entry
/// that cannot be read or a changed directory is the error: a size nobody
/// measured is not reported as zero.
fn tree_bytes(dir_fd: RawFd, device: nix::libc::dev_t) -> Result<i64, JanitorError> {
    let mut total = 0i64;
    for name in entry_names(dir_fd)? {
        let info = safefs::fstatat_nofollow(dir_fd, &name)?;
        #[allow(clippy::unnecessary_cast)] // st_mode is u16 on macOS
        if ifmt(info.st_mode as u32) == IFDIR {
            if info.st_dev != device {
                return Err(JanitorError::os(&format!(
                    "{name:?} is another volume mounted inside a Cargo build directory"
                )));
            }
            let child = safefs::open_dir_at(dir_fd, &name)?;
            if !same_object(&safefs::fstat(child.as_raw_fd())?, &info) {
                return Err(JanitorError::os(&format!(
                    "{name:?} was replaced while it was measured"
                )));
            }
            total = total.saturating_add(tree_bytes(child.as_raw_fd(), device)?);
        } else {
            total = total.saturating_add(info.st_blocks.saturating_mul(512));
        }
    }
    Ok(total)
}

/// The child directories of `dir_fd` this account owns on `device`; links,
/// files and other accounts' entries are passed over.
fn own_directories(dir_fd: RawFd, device: nix::libc::dev_t) -> Result<Vec<OsString>, JanitorError> {
    let mut directories = Vec::new();
    for name in entry_names(dir_fd)? {
        let info = safefs::fstatat_nofollow(dir_fd, &name)?;
        #[allow(clippy::unnecessary_cast)] // st_mode is u16 on macOS
        if ifmt(info.st_mode as u32) == IFDIR && info.st_uid == euid() && info.st_dev == device {
            directories.push(name);
        }
    }
    Ok(directories)
}

/// One pass over `$CARGO_HOME/build/<xx>/<hash>`: each directory measured,
/// and in an enforcing pass removed unless a build holds it or it lies
/// outside the tree this host lets the janitor delete in.
pub(in crate::providers::local::space_reclaim) fn scan_cargo_build_dirs(
    home: &Path,
    enforcing: bool,
    deletable: Option<&Path>,
    lock_dir: &Path,
    writer: &str,
    report: &mut CleanupReport,
) {
    let root = cargo_build_root(home);
    let root_fd = match safefs::open_dir_path(&root) {
        Ok(root_fd) => root_fd,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            report.skip_cargo_builds("root_absent", 1);
            return;
        }
        Err(error) => {
            report.add_error(
                CARGO_BUILD_DIRS,
                &JanitorError::os(&format!(
                    "Cargo's build directory {} cannot be opened: {error}",
                    root.display()
                )),
            );
            return;
        }
    };
    let body = |report: &mut CleanupReport| -> Result<(), JanitorError> {
        let root_info = safefs::fstat(root_fd.as_raw_fd())?;
        if root_info.st_uid != euid() {
            return Err(JanitorError::os(&format!(
                "{} is not this account's directory, so nothing in it is taken",
                root.display()
            )));
        }
        let device = root_info.st_dev;
        let running = std::env::current_exe().ok();
        for shard in own_directories(root_fd.as_raw_fd(), device)? {
            let shard_fd = safefs::open_dir_at(root_fd.as_raw_fd(), &shard)?;
            for name in own_directories(shard_fd.as_raw_fd(), device)? {
                let path = root.join(&shard).join(&name);
                let dir_fd = safefs::open_dir_at(shard_fd.as_raw_fd(), &name)?;
                report.cargo_builds.scanned_items += 1;
                report.cargo_builds.eligible_items += 1;
                let bytes = match tree_bytes(dir_fd.as_raw_fd(), device) {
                    Ok(bytes) => bytes,
                    Err(mut error) => {
                        error.message =
                            format!("cannot measure {}: {}", path.display(), error.message);
                        report.add_error(CARGO_BUILD_DIRS, &error);
                        continue;
                    }
                };
                report.cargo_builds.expected_bytes += bytes;
                // The size of a build this host makes: the room a pass
                // leaves under the rule for the next one (`margin`).
                if report
                    .largest_cargo_build
                    .as_ref()
                    .is_none_or(|(_, largest)| bytes > *largest)
                {
                    report.largest_cargo_build = Some((path.clone(), bytes));
                }
                if !enforcing {
                    continue;
                }
                if !ownership::may_delete(deletable, &path) {
                    report.skip_cargo_builds(ownership::NOT_STADO_OWNED, 1);
                    continue;
                }
                // A Stado run out of a build directory does not take the
                // directory its own executable is in.
                if running
                    .as_ref()
                    .is_some_and(|running| running.starts_with(&path))
                {
                    report.skip_cargo_builds("running_executable", 1);
                    continue;
                }
                let _cleanup = match acquire_lock_state(lock_dir, writer)? {
                    LockState::Held(lock) => {
                        if !retired_locks_active(lock_dir, &lock.file)?.is_empty() {
                            report.skip_cargo_builds("lock_predecessor_active", 1);
                            continue;
                        }
                        lock
                    }
                    LockState::Busy { .. } => {
                        report.skip_cargo_builds("cleanup_lock_busy", 1);
                        continue;
                    }
                };
                let _held = match held::take(dir_fd.as_raw_fd())? {
                    held::Holding::Free(locks) => locks,
                    held::Holding::Busy(_) => {
                        report.skip_cargo_builds("held_by_build", 1);
                        continue;
                    }
                };
                let before = free_bytes(home)?;
                match remove_tree(shard_fd.as_raw_fd(), &name, dir_fd.as_raw_fd(), device) {
                    Ok(()) => {
                        let freed = (free_bytes(home)? - before).max(0);
                        report.cargo_builds.actual_free_delta_bytes += freed;
                        report.cargo_builds.deleted_items += 1;
                        if let Err(error) = ledger::record_tree(
                            lock_dir,
                            CARGO_BUILD_DIRS,
                            writer,
                            &path,
                            bytes,
                            freed,
                        ) {
                            report.add_error("removal_record", &error);
                        }
                    }
                    Err(mut error) => {
                        error.message =
                            format!("cannot remove {}: {}", path.display(), error.message);
                        report.add_error(CARGO_BUILD_DIRS, &error);
                    }
                }
            }
        }
        Ok(())
    };
    if let Err(error) = body(report) {
        report.add_error(CARGO_BUILD_DIRS, &error);
    }
}
