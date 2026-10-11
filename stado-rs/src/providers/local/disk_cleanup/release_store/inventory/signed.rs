//! Whether one version directory holds a complete signed release on disk.

use std::path::Path;

use crate::providers::local::disk_cleanup::release_store::SIGNED_RELEASE;

/// True when at least one platform directory of this version holds every
/// object of [`SIGNED_RELEASE`]. All of them from ONE platform directory: two
/// of three is what an interrupted publish leaves, and an installer reads the
/// manifest and then verifies the archive against its digest, so a version
/// missing either fails after the download instead of before it. A version
/// directory that cannot be listed is the error: read as incomplete, an
/// older version could take its place as the newest installable one.
pub(in crate::providers::local::disk_cleanup::release_store) fn signed_release_complete(
    version_path: &Path,
) -> std::io::Result<bool> {
    let platforms = std::fs::read_dir(version_path)?;
    Ok(platforms.flatten().any(|platform| {
        let platform_path = platform.path();
        platform_path.is_dir()
            && SIGNED_RELEASE
                .iter()
                .all(|name| platform_path.join(name).is_file())
    }))
}
