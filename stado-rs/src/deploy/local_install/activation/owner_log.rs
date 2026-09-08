//! The owner-only log a loaded job writes its receipts into. Prepared before
//! the job is booted, so the first line launchd or cron produces has a file
//! to land in that no other account can read or replace.

use std::fs::{self, OpenOptions};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use crate::deploy::DeployError;

/// `pub(super)` for [`super::execute_plan`] and [`super::cron`], the two
/// activation paths that boot a job and must hand it a log path first.
pub(super) fn prepare_owner_log(home: &Path, label: &str) -> Result<PathBuf, DeployError> {
    let directory = home.join(".stado").join("logs");
    match fs::symlink_metadata(&directory) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            return Err(DeployError(format!(
                "refusing non-directory agent log path {}",
                directory.display()
            )));
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir_all(&directory).map_err(|error| DeployError(error.to_string()))?;
        }
        Err(error) => return Err(DeployError(error.to_string())),
    }
    #[allow(clippy::unnecessary_cast)] // mode_t is u16 on macOS, u32 on Linux
    let directory_mode = nix::libc::S_IRWXU as u32;
    fs::set_permissions(&directory, fs::Permissions::from_mode(directory_mode))
        .map_err(|error| DeployError(error.to_string()))?;

    let log = directory.join(format!("{label}.log"));
    match fs::symlink_metadata(&log) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            return Err(DeployError(format!(
                "refusing non-file agent log path {}",
                log.display()
            )));
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(DeployError(error.to_string())),
    }
    #[allow(clippy::unnecessary_cast)] // mode_t is u16 on macOS, u32 on Linux
    let file_mode = (nix::libc::S_IRUSR | nix::libc::S_IWUSR) as u32;
    OpenOptions::new()
        .create(true)
        .append(true)
        .mode(file_mode)
        .open(&log)
        .map_err(|error| DeployError(error.to_string()))?;
    fs::set_permissions(&log, fs::Permissions::from_mode(file_mode))
        .map_err(|error| DeployError(error.to_string()))?;
    Ok(log)
}
