//! The fixed roots a cleaner may walk, and the free space a pass measures.

use std::ffi::OsString;
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use crate::providers::local::disk_cleanup::janitor::pass::lock::euid;
use crate::providers::local::disk_cleanup::janitor::state::error::JanitorError;

// ---------------------------------------------------------------------------
// fixed roots + free space
// ---------------------------------------------------------------------------

/// Python `_fixed_root`: walk `parts` beneath `home`, requiring every
/// component to be a non-symlink directory owned by us on home's device;
/// the resolved root must stay strictly beneath home.
pub fn fixed_root(
    home: &Path,
    parts: &[OsString],
    required: bool,
) -> Result<Option<PathBuf>, JanitorError> {
    let mut current = home.to_path_buf();
    let home_device = std::fs::metadata(home)?.dev();
    for part in parts {
        current = current.join(part);
        let info = match std::fs::symlink_metadata(&current) {
            Ok(info) => info,
            Err(exc) if exc.kind() == io::ErrorKind::NotFound => {
                if required {
                    return Err(JanitorError::from(exc));
                }
                return Ok(None);
            }
            Err(exc) => return Err(exc.into()),
        };
        if info.file_type().is_symlink() || !info.is_dir() {
            return Err(JanitorError::os("unsafe cleaner root"));
        }
        if info.uid() != euid() || info.dev() != home_device {
            return Err(JanitorError::os(
                "cleaner root ownership or device mismatch",
            ));
        }
    }
    let resolved = std::fs::canonicalize(&current)?;
    if resolved == home || !resolved.starts_with(home) {
        return Err(JanitorError::os("cleaner root is not beneath home"));
    }
    Ok(Some(resolved))
}

/// Resolve a configured, home-contained scan root through the same ownership
/// and non-symlink checks as a fixed root.
pub fn configured_root(
    home: &Path,
    configured: Option<&str>,
    defaults: &[OsString],
    required: bool,
) -> Result<Option<PathBuf>, JanitorError> {
    let Some(configured) = configured else { return fixed_root(home, defaults, required); };
    let expanded = crate::config_file::expand_tilde(configured);
    let relative = expanded.strip_prefix(home)
        .map_err(|_| JanitorError::os("cleaner root must be beneath the host home"))?;
    let parts = relative.components().map(|part| match part {
        std::path::Component::Normal(name) => Ok(name.to_os_string()),
        _ => Err(JanitorError::os("cleaner root must contain only normal path components")),
    }).collect::<Result<Vec<_>, _>>()?;
    fixed_root(home, &parts, required)
}

/// Python `_free_bytes` (`shutil.disk_usage(home).free`).
pub fn free_bytes(home: &Path) -> Result<i64, JanitorError> {
    let stat = nix::sys::statvfs::statvfs(home)
        .map_err(|e| JanitorError::from(io::Error::from_raw_os_error(e as i32)))?;
    Ok((stat.blocks_available() as i64) * (stat.fragment_size() as i64))
}
