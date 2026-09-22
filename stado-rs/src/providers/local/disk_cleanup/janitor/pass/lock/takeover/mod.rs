//! Acquiring the exclusive lock, including the bounded takeover of a
//! holder past its own declared deadline.

use std::os::unix::fs::MetadataExt;
use std::path::Path;

use crate::providers::local::disk_cleanup::janitor::pass::lock::file::{
    lock_contended, lock_token, open_lock, open_lock_at, read_lock_holder, write_lock_holder,
    ExclusiveLock, LockState,
};
use crate::providers::local::disk_cleanup::janitor::state::error::JanitorError;
use crate::providers::local::disk_cleanup::janitor::{
    LOCK_NAME, RETIRED_LOCK_PREFIX, TAKEOVER_LOCK_NAME,
};

mod holder;
mod overdue;

use holder::path_names_file;
pub(crate) use holder::{pid_alive, retired_locks_active};
use overdue::overdue_predecessor;

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
