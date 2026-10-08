//! A Swift package's resolved dependencies as one release input.
//!
//! A desktop release builds with `swift build --disable-automatic-resolution`
//! on a worker that holds no Git credentials for the private packages, so its
//! release script unpacks this archive into the source before building: the
//! `.build/` SwiftPM resolves into — `checkouts/`, `repositories/`,
//! `artifacts/` and `workspace-state.json` — exactly as `Package.resolved`
//! pins it. Every desktop pinned such an archive by hand in August; the
//! fleet store no longer held one when the store moved, and nothing could
//! make another (b6996b35).

use std::path::Path;
use std::process::Command;

use flate2::{Compression, GzBuilder};

use crate::cli::CmdError;

/// Where SwiftPM keeps a package's resolution, relative to the package root,
/// and so where the release script's unpacking puts it back.
const SCRATCH: &str = ".build";

/// Resolve `package` as its committed `Package.resolved` pins it, into a
/// stage under `scratch`, and pack that resolution as a deterministic
/// gzip-compressed tar whose entries start at `.build/`.
pub(super) fn export(package: &Path, scratch: &Path) -> Result<tempfile::NamedTempFile, CmdError> {
    std::fs::create_dir_all(scratch)?;
    let stage = tempfile::tempdir_in(scratch)?;
    let resolved = stage.path().join(SCRATCH);
    let output = Command::new("swift")
        .arg("package")
        .arg("--package-path")
        .arg(package)
        .arg("--scratch-path")
        .arg(&resolved)
        // Package.resolved is the answer; a dependency it does not pin is a
        // refusal, not a newer resolution published as the release's input.
        .arg("--disable-automatic-resolution")
        .arg("resolve")
        .output()
        .map_err(|error| {
            CmdError::click(format!(
                "cannot run swift package resolve for {}: {error}; no input was published",
                package.display()
            ))
        })?;
    if !output.status.success() {
        return Err(CmdError::click(format!(
            "swift package resolve in {} failed ({}): {}{}; no input was published",
            package.display(),
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )));
    }
    let archive = tempfile::NamedTempFile::new_in(scratch)?;
    let encoder = GzBuilder::new().write(archive.reopen()?, Compression::best());
    let mut tar = tar::Builder::new(encoder);
    tar.mode(tar::HeaderMode::Deterministic);
    // A checkout may carry symlinks of its own; they are stored as links.
    tar.follow_symlinks(false);
    tar.append_dir(SCRATCH, &resolved)?;
    append(&mut tar, stage.path(), Path::new(SCRATCH))?;
    tar.into_inner()?.finish()?.sync_all()?;
    stage.close().map_err(|error| {
        CmdError::click(format!(
            "cannot remove the SwiftPM resolution stage under {}: {error}; no input was published",
            scratch.display()
        ))
    })?;
    Ok(archive)
}

/// Every entry under `root/relative`, in name order, so the same resolution
/// gives the same bytes.
fn append<W: std::io::Write>(
    tar: &mut tar::Builder<W>,
    root: &Path,
    relative: &Path,
) -> std::io::Result<()> {
    let mut entries =
        std::fs::read_dir(root.join(relative))?.collect::<std::io::Result<Vec<_>>>()?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let path = relative.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            tar.append_dir(&path, entry.path())?;
            append(tar, root, &path)?;
        } else {
            tar.append_path_with_name(entry.path(), &path)?;
        }
    }
    Ok(())
}
