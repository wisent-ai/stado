//! The fixed roots a cleaner may walk, and the free space a pass measures.

use std::ffi::OsString;
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use crate::providers::local::space_reclaim::janitor::pass::lock::euid;
use crate::providers::local::space_reclaim::janitor::state::error::JanitorError;

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

/// The bytes an unprivileged writer may still take on the volume holding
/// `home`, as the disk-full rule reads that volume (saturating where the
/// plain product overflowed).
pub fn free_bytes(home: &Path) -> Result<i64, JanitorError> {
    Ok(crate::providers::local::space_reclaim::rule::read_volume(home)?.free_bytes)
}
