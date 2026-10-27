//! The secure home, the state directory, and the lock files beneath it.

pub(crate) mod file;
pub(crate) mod holds;
pub(crate) mod ownership;
pub(crate) mod workload;

use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use crate::providers::local::space_reclaim::janitor::state::error::JanitorError;
use crate::providers::local::space_reclaim::janitor::STATE_DIR_PARTS;

// ---------------------------------------------------------------------------
// secure home / state dir / lock file
// ---------------------------------------------------------------------------

/// Python `_secure_home`: the home must be a real (non-symlink) directory
/// owned by the effective uid; returned fully resolved.
pub fn secure_home(home: &Path) -> Result<PathBuf, JanitorError> {
    let info = std::fs::symlink_metadata(home)?;
    if info.file_type().is_symlink() || !info.is_dir() {
        return Err(JanitorError::os("unsafe home"));
    }
    if info.uid() != euid() {
        return Err(JanitorError::os("home owner mismatch"));
    }
    Ok(std::fs::canonicalize(home)?)
}

/// The account's home as every Stado file is kept under
/// ([`crate::config_file::home`]): HOME, else the user database; with
/// neither, the cause, where the janitor used to stop the process.
pub fn user_home() -> Result<PathBuf, JanitorError> {
    crate::config_file::home().map_err(|cause| JanitorError::os(&cause))
}

/// The process effective uid (Python `os.geteuid()`).
pub(crate) fn euid() -> u32 {
    // SAFETY: geteuid cannot fail.
    unsafe { nix::libc::geteuid() }
}

/// Python `_ensure_state_dir`: create `~/.cache/wisent-compute` component
/// by component (mode 0700), refusing symlinks and foreign owners.
pub fn ensure_state_dir(home: &Path) -> Result<PathBuf, JanitorError> {
    let mut current = home.to_path_buf();
    for component in STATE_DIR_PARTS {
        current = current.join(component);
        let info = match std::fs::symlink_metadata(&current) {
            Ok(info) => info,
            Err(exc) if exc.kind() == io::ErrorKind::NotFound => {
                std::fs::create_dir(&current)?;
                // Owner-only, explicit like fchmod would be, whatever the
                // umask allowed.
                use std::os::unix::fs::PermissionsExt;
                let mode = crate::primitives::file_mode::owner_only_directory();
                std::fs::set_permissions(&current, std::fs::Permissions::from_mode(mode))?;
                std::fs::symlink_metadata(&current)?
            }
            Err(exc) => return Err(exc.into()),
        };
        if info.file_type().is_symlink() || !info.is_dir() {
            return Err(JanitorError::os("unsafe state directory"));
        }
        if info.uid() != euid() {
            return Err(JanitorError::os("state directory owner mismatch"));
        }
    }
    Ok(current)
}
