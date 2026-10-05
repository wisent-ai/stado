//! Whether one version directory holds a complete signed release on disk.

use std::path::Path;

use crate::providers::local::disk_cleanup::release_store::SIGNED_RELEASE;

/// True when at least one platform directory of this version holds every
/// object of [`SIGNED_RELEASE`]. All of them from ONE platform directory: two
/// of three is what an interrupted publish leaves, and an installer reads the
/// manifest and then verifies the archive against its digest, so a version
/// missing either fails after the download instead of before it.
pub(in crate::providers::local::disk_cleanup::release_store) fn signed_release_complete(
    version_path: &Path,
) -> bool {
    let Ok(platforms) = std::fs::read_dir(version_path) else {
        return false;
    };
    platforms.flatten().any(|platform| {
        let platform_path = platform.path();
        platform_path.is_dir()
            && SIGNED_RELEASE
                .iter()
                .all(|name| platform_path.join(name).is_file())
    })
}
