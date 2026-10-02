//! Nonblocking kernel ownership and protection of already-retired lock inodes.

use std::fs::{File, OpenOptions};
use std::io;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::Path;

use crate::providers::local::disk_cleanup::janitor::pass::lock::euid;
use crate::providers::local::disk_cleanup::janitor::pass::lock::file::{
    holder_inode_record_path, lock_contended, open_lock, read_lock_holder, write_lock_holder,
    ExclusiveLock, LockState,
};
use crate::providers::local::disk_cleanup::janitor::state::error::JanitorError;
use crate::providers::local::disk_cleanup::janitor::state::report::build::epoch_now;
use crate::providers::local::disk_cleanup::janitor::{LOCK_NAME, RETIRED_LOCK_PREFIX};

pub(crate) fn acquire_lock_state(
    state_dir: &Path,
    writer: &str,
) -> Result<LockState, JanitorError> {
    let file = open_lock(state_dir)?;
    match fs2::FileExt::try_lock_exclusive(&file) {
        Ok(()) => {
            let canonical = state_dir.join(LOCK_NAME);
            if !path_names_file(&canonical, &file)? {
                return Err(JanitorError::os(&format!(
                    "cleanup lock {} no longer names the opened lock inode; no pass was started",
                    canonical.display()
                )));
            }
            let (token, records) = write_lock_holder(state_dir, &file, writer)?;
            Ok(LockState::Held(ExclusiveLock {
                file,
                holder_records: records,
                holder_token: token,
            }))
        }
        Err(error) if lock_contended(&error) => Ok(LockState::Busy {
            holder: read_lock_holder(state_dir, &file),
        }),
        Err(error) => Err(error.into()),
    }
}

/// Is a pid still a process on this host?
///
/// `kill(pid, 0)` answers exactly that and nothing else: `ESRCH` means gone,
/// `EPERM` means alive and owned by somebody else. This observes process
/// existence; it neither measures progress nor releases a kernel lock.
pub(crate) fn pid_alive(pid: i32) -> bool {
    if pid <= 0 {
        return false;
    }
    match unsafe { nix::libc::kill(pid, 0) } {
        0 => true,
        _ => io::Error::last_os_error().raw_os_error() == Some(nix::libc::EPERM),
    }
}

fn open_existing_lock_at(path: &Path) -> Result<File, JanitorError> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(nix::libc::O_NOFOLLOW)
        .open(path)?;
    let info = file.metadata()?;
    if !info.is_file() || info.uid() != euid() {
        return Err(JanitorError::os("unsafe cleanup lock file"));
    }
    Ok(file)
}

fn path_names_file(path: &Path, file: &File) -> Result<bool, JanitorError> {
    let path_info = std::fs::symlink_metadata(path).map_err(|error| {
        JanitorError::os(&format!(
            "cannot inspect cleanup lock {}: {error}",
            path.display()
        ))
    })?;
    let file_info = file.metadata().map_err(|error| {
        JanitorError::os(&format!(
            "cannot inspect opened cleanup lock {}: {error}",
            path.display()
        ))
    })?;
    Ok(!path_info.file_type().is_symlink()
        && path_info.is_file()
        && path_info.uid() == euid()
        && path_info.dev() == file_info.dev()
        && path_info.ino() == file_info.ino())
}

/// Who still holds a retired predecessor lock, said in one line.
///
/// A contended retired inode is a live process: the kernel releases an
/// `flock` the moment its holder dies, so a lock that refuses this pass is
/// held by something that is running now. Saying only "a retired cleanup lock
/// inode is still held" and stopping would leave every pass persisting
/// diagnostics and deleting nothing while the host sits below its disk target
/// — with no way to learn which process to look at.
fn holder_sentence(state_dir: &Path, file: &File) -> String {
    let Some(holder) = read_lock_holder(state_dir, file) else {
        return "a kernel lock with no readable holder record".to_string();
    };
    let age = epoch_now() - holder.acquired_at;
    let living = if pid_alive(holder.pid) {
        "running"
    } else {
        "gone, but its lock is still held by a process that inherited it"
    };
    format!(
        "recorded pid {} ({} {}), acquired {age:.0}s ago, {living}; the kernel lock remains held",
        holder.pid, holder.writer, holder.writer_version
    )
}

/// Check every retired predecessor inode while holding the current exclusive
/// lock. An unlocked predecessor is removed; a still-locked one keeps this
/// pass report-only so two lock generations can never authorize deletion at
/// the same time — and names who is holding it.
pub(crate) fn retired_locks_active(
    state_dir: &Path,
    current_lock: &File,
) -> Result<Vec<String>, JanitorError> {
    let current_info = current_lock.metadata()?;
    let mut holders = Vec::new();
    for entry in std::fs::read_dir(state_dir)? {
        let entry = entry?;
        let name = entry.file_name();
        if !name.to_string_lossy().starts_with(RETIRED_LOCK_PREFIX) {
            continue;
        }
        let path = entry.path();
        let file = open_existing_lock_at(&path)?;
        let info = file.metadata()?;
        if info.dev() == current_info.dev() && info.ino() == current_info.ino() {
            // A contender can die after creating the hard link but before
            // replacing the canonical pathname. This process owns that same
            // inode exclusively, so the extra name is safe to remove.
            if path_names_file(&path, &file)? {
                std::fs::remove_file(path)?;
            }
            continue;
        }
        match fs2::FileExt::try_lock_exclusive(&file) {
            Ok(()) => {
                let still_named = path_names_file(&path, &file)?;
                let stale_holder = holder_inode_record_path(state_dir, &file)?;
                fs2::FileExt::unlock(&file)?;
                if still_named {
                    std::fs::remove_file(path)?;
                    let _ = std::fs::remove_file(stale_holder);
                }
            }
            Err(error) if lock_contended(&error) => holders.push(holder_sentence(state_dir, &file)),
            Err(error) => return Err(error.into()),
        }
    }
    Ok(holders)
}
