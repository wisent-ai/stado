//! Acquiring the exclusive lock, including the bounded takeover of a
//! holder past its own declared deadline.

use std::fs::{File, OpenOptions};
use std::io;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::Path;

use crate::providers::local::disk_cleanup::janitor::pass::lock::euid;
use crate::providers::local::disk_cleanup::janitor::pass::lock::file::{
    holder_inode_record_path, lock_contended, lock_token, open_lock, open_lock_at,
    read_lock_holder, write_lock_holder, ExclusiveLock, LockHolder, LockState,
};
use crate::providers::local::disk_cleanup::janitor::state::error::JanitorError;
use crate::providers::local::disk_cleanup::janitor::state::report::build::epoch_now;
use crate::providers::local::disk_cleanup::janitor::{
    LOCK_NAME, LOCK_TAKEOVER_GRACE_S, RETIRED_LOCK_PREFIX, TAKEOVER_LOCK_NAME,
};

/// Is a pid still a process on this host?
///
/// `kill(pid, 0)` answers exactly that and nothing else: `ESRCH` means gone,
/// `EPERM` means alive and owned by somebody else. A gone holder's `flock` is
/// already released by the kernel, so this is a diagnosis rather than a
/// release mechanism — it is what lets the report distinguish "the holder
/// died mid-pass" from "the holder is hung".
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

fn path_names_file(path: &Path, file: &File) -> bool {
    let Ok(path_info) = std::fs::symlink_metadata(path) else {
        return false;
    };
    let Ok(file_info) = file.metadata() else {
        return false;
    };
    !path_info.file_type().is_symlink()
        && path_info.is_file()
        && path_info.uid() == euid()
        && path_info.dev() == file_info.dev()
        && path_info.ino() == file_info.ino()
}

/// Check every retired predecessor inode while holding the current exclusive
/// lock. An unlocked predecessor is removed; a still-locked one keeps this
/// pass report-only so two lock generations can never authorize deletion at
/// the same time.
pub(crate) fn retired_locks_active(
    state_dir: &Path,
    current_lock: &File,
) -> Result<bool, JanitorError> {
    let current_info = current_lock.metadata()?;
    let mut active = false;
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
            if path_names_file(&path, &file) {
                std::fs::remove_file(path)?;
            }
            continue;
        }
        match fs2::FileExt::try_lock_exclusive(&file) {
            Ok(()) => {
                let still_named = path_names_file(&path, &file);
                let stale_holder = holder_inode_record_path(state_dir, &file)?;
                fs2::FileExt::unlock(&file)?;
                if still_named {
                    std::fs::remove_file(path)?;
                    let _ = std::fs::remove_file(stale_holder);
                }
            }
            Err(error) if lock_contended(&error) => active = true,
            Err(error) => return Err(error.into()),
        }
    }
    Ok(active)
}

fn overdue_holder(state_dir: &Path, lock: &File) -> Option<(LockHolder, f64)> {
    let holder = read_lock_holder(state_dir, lock)?;
    let overdue_seconds = epoch_now() - (holder.deadline_at + LOCK_TAKEOVER_GRACE_S);
    (overdue_seconds > 0.0).then_some((holder, overdue_seconds))
}

/// How long the lock file itself says the current hold has lasted.
///
/// The holder record is the precise answer and this is the observable one: the
/// canonical lock file is rewritten every time the lock changes hands, so its
/// modification time bounds the age of the hold that exists right now.
fn unattributed_overdue(lock: &File, pass_seconds: f64) -> Option<f64> {
    let modified = lock.metadata().ok()?.modified().ok()?;
    let age = std::time::SystemTime::now()
        .duration_since(modified)
        .ok()?
        .as_secs_f64();
    let allowance = pass_seconds + LOCK_TAKEOVER_GRACE_S;
    (age > allowance).then_some(age - allowance)
}

/// The predecessor this pass may take the lock from, recorded or not.
///
/// A hold with no holder record used to be permanent: `overdue_holder` reads
/// the record first and answers `None` without it, so the pass reported
/// `lock_busy_unattributed` and left. On `charless-mac-mini` on 2026-09-06
/// that state lasted two hours and counting - the agent's own janitor tick
/// held the kernel lock with no record, every later tick declined to take it,
/// and the host stopped reclaiming anything at 5.3 GiB free while fifty
/// queued documentation records waited for space that only this janitor
/// returns. A missing record is not a live budget: the lock file's own age is
/// the evidence that exists in every case, and a hold past the declared pass
/// deadline is overdue whether or not its owner wrote itself down.
fn overdue_predecessor(
    state_dir: &Path,
    lock: &File,
    pass_seconds: f64,
) -> Option<(Option<LockHolder>, f64)> {
    if let Some((holder, overdue_seconds)) = overdue_holder(state_dir, lock) {
        return Some((Some(holder), overdue_seconds));
    }
    if read_lock_holder(state_dir, lock).is_some() {
        return None;
    }
    unattributed_overdue(lock, pass_seconds).map(|overdue_seconds| (None, overdue_seconds))
}

/// Exclusive flock, with a bounded and mutually exclusive recovery path.
///
/// A takeover keeps a hard link to the predecessor inode before atomically
/// replacing the canonical pathname. Every later cleanup probes that retired
/// inode and remains report-only until its kernel lock is released. The short
/// hard-link/rename section has its own mutex, preventing two contenders from
/// retiring different generations concurrently.
pub(crate) fn acquire_lock_state(
    state_dir: &Path,
    pass_seconds: f64,
    writer: &str,
) -> Result<LockState, JanitorError> {
    let canonical = state_dir.join(LOCK_NAME);
    let file = open_lock(state_dir)?;
    match fs2::FileExt::try_lock_exclusive(&file) {
        Ok(()) => {
            let (token, records) = write_lock_holder(state_dir, &file, pass_seconds, writer)?;
            return Ok(LockState::Held(ExclusiveLock {
                file,
                holder_records: records,
                holder_token: Some(token),
            }));
        }
        Err(error) if lock_contended(&error) => {}
        Err(error) => return Err(error.into()),
    }
    let initial_holder = read_lock_holder(state_dir, &file);
    if overdue_predecessor(state_dir, &file, pass_seconds).is_none() {
        return Ok(LockState::Busy {
            holder: initial_holder,
        });
    }

    let takeover_file = open_lock_at(&state_dir.join(TAKEOVER_LOCK_NAME))?;
    match fs2::FileExt::try_lock_exclusive(&takeover_file) {
        Ok(()) => {}
        Err(error) if lock_contended(&error) => {
            return Ok(LockState::Busy {
                holder: initial_holder,
            });
        }
        Err(error) => return Err(error.into()),
    }
    let _takeover_guard = ExclusiveLock {
        file: takeover_file,
        holder_records: Vec::new(),
        holder_token: None,
    };

    // Re-open and re-evaluate after winning the takeover mutex. Another
    // contender may have replaced or released the lock while we waited.
    let current = open_lock(state_dir)?;
    match fs2::FileExt::try_lock_exclusive(&current) {
        Ok(()) => {
            let (token, records) = write_lock_holder(state_dir, &current, pass_seconds, writer)?;
            return Ok(LockState::Held(ExclusiveLock {
                file: current,
                holder_records: records,
                holder_token: Some(token),
            }));
        }
        Err(error) if lock_contended(&error) => {}
        Err(error) => return Err(error.into()),
    }
    let Some((holder, overdue_seconds)) = overdue_predecessor(state_dir, &current, pass_seconds)
    else {
        return Ok(LockState::Busy {
            holder: read_lock_holder(state_dir, &current),
        });
    };

    let replacement_token = lock_token();
    let current_info = current.metadata()?;
    let retired = state_dir.join(format!(
        "{RETIRED_LOCK_PREFIX}{}.{}.{}",
        current_info.dev(),
        current_info.ino(),
        replacement_token
    ));
    std::fs::hard_link(&canonical, &retired)?;
    if !path_names_file(&canonical, &current) {
        let _ = std::fs::remove_file(&retired);
        return Ok(LockState::Busy { holder });
    }

    let staged = state_dir.join(format!(".{LOCK_NAME}.takeover.{replacement_token}"));
    let fresh = open_lock_at(&staged)?;
    if let Err(error) = fs2::FileExt::try_lock_exclusive(&fresh) {
        let _ = std::fs::remove_file(&staged);
        let _ = std::fs::remove_file(&retired);
        return Err(error.into());
    }
    if let Err(error) = std::fs::rename(&staged, &canonical) {
        let _ = std::fs::remove_file(&staged);
        let _ = std::fs::remove_file(&retired);
        return Err(error.into());
    }
    let (token, records) = match write_lock_holder(state_dir, &fresh, pass_seconds, writer) {
        Ok(holder) => holder,
        Err(error) => {
            // Put the predecessor inode back under the canonical name. Its
            // holder record still describes it, so the next pass can retry.
            let _ = std::fs::rename(&retired, &canonical);
            return Err(error);
        }
    };
    Ok(LockState::TakenOver {
        lock: ExclusiveLock {
            file: fresh,
            holder_records: records,
            holder_token: Some(token),
        },
        from_pid: holder.map_or(0, |holder| holder.pid),
        overdue_seconds,
    })
}
