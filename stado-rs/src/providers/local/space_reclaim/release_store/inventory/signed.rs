//! Whether one version directory holds a complete signed release on disk.

use std::path::Path;

use crate::providers::local::space_reclaim::release_store::SIGNED_RELEASE;

/// True when at least one platform directory of this version holds every
/// object of [`SIGNED_RELEASE`]. All of them from ONE platform directory: two
/// of three is what an interrupted publish leaves, and an installer reads the
/// manifest and then verifies the archive against its digest, so a version
/// missing either fails after the download instead of before it. A version
/// directory, a platform entry or an object that cannot be read is the
/// error: read as incomplete, an older version could take its place as the
/// newest installable one.
pub(in crate::providers::local::space_reclaim::release_store) fn signed_release_complete(
    version_path: &Path,
) -> std::io::Result<bool> {
    for platform in std::fs::read_dir(version_path)? {
        let platform_path = platform?.path();
        if !is_kind(&platform_path, std::fs::Metadata::is_dir)? {
            continue;
        }
        let mut complete = true;
        for name in SIGNED_RELEASE {
            complete &= is_kind(&platform_path.join(name), std::fs::Metadata::is_file)?;
        }
        if complete {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Whether `path` is there and of the kind `kind` reads; absent is not, and
/// any other failure to read it is the error.
fn is_kind(path: &Path, kind: fn(&std::fs::Metadata) -> bool) -> std::io::Result<bool> {
    match std::fs::metadata(path) {
        Ok(metadata) => Ok(kind(&metadata)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}
