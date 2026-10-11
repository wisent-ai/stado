//! A Swift package's resolved dependencies as one release input.
//!
//! A desktop release builds with `swift build --disable-automatic-resolution`
//! on a worker that holds no Git credentials for the private packages, so its
//! release preparation restores this archive into the source before building.
//! Artifact paths are relative to `.build/` in the archive; Stado resolves them
//! against the worker's actual cache location when restoring.

use std::path::Path;
use std::process::Command;

use flate2::{Compression, GzBuilder};

use crate::cli::CmdError;

/// Where SwiftPM keeps a package's resolution, relative to the package root,
/// and so where the release script's unpacking puts it back.
const SCRATCH: &str = ".build";

/// What every stage and archive this verb leaves under the scratch is named
/// with, followed by the creating process id and a dash.
const STAGE_PREFIX: &str = "swiftpm-";

/// Remove the stages and archives of publications whose process is gone. A
/// resolution is gigabytes, and an interrupted run cannot remove its own:
/// two interrupted publications of most-desktop left 2.7 GB under
/// `.build/release-input` that nothing would ever read again.
fn sweep_abandoned(scratch: &Path) -> Result<(), CmdError> {
    for entry in std::fs::read_dir(scratch)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(pid) = name
            .strip_prefix(STAGE_PREFIX)
            .and_then(|rest| rest.split('-').next())
            .and_then(|pid| pid.parse::<i32>().ok())
        else {
            continue;
        };
        if crate::providers::local::helpers::running_slot::pid_alive(pid) {
            continue;
        }
        let path = entry.path();
        let removed = if entry.file_type()?.is_dir() {
            std::fs::remove_dir_all(&path)
        } else {
            std::fs::remove_file(&path)
        };
        removed.map_err(|error| {
            CmdError::click(format!(
                "cannot remove the abandoned SwiftPM stage {}: {error}",
                path.display()
            ))
        })?;
    }
    Ok(())
}

/// What SwiftPM writes when the shell running it is itself sandboxed and its
/// manifest sandbox cannot nest inside it. The refusal stays a refusal: the
/// manifest is never evaluated unsandboxed, and a second resolution with
/// SwiftPM's sandbox off would publish bytes no sandbox had watched being
/// made. What is named instead is the route that runs unsandboxed.
const NESTED_SANDBOX_CAUSE: &str = "sandbox_apply: Operation not permitted";

/// The way out of a nested-sandbox refusal: the same verb through the Stado
/// service, whose process is not the sandboxed shell. The sentence names both
/// of the service's surfaces and what the service needs to read the checkout,
/// so the next refusal met on that route is already explained here.
const NESTED_SANDBOX_WAY_OUT: &str = "; this shell is sandboxed and SwiftPM's manifest sandbox cannot nest inside it: run pin-input from a shell outside the sandbox, or have the Stado service run it, which is unsandboxed - Stado Desktop, Releases, \"Publish an immutable build input\", or POST /api/operator/run on the service's API with X-Stado-Action: operator-command, args [\"release\",\"catalog\",\"pin-input\",...] and confirmation RUN_MUTATION; the service's process must be allowed to read the checkout and the source (on macOS, the folder access the system grants it)";

/// Run `swift package resolve` for `package` into `resolved`, as its committed
/// Package.resolved pins it.
fn resolve(package: &Path, resolved: &Path) -> Result<std::process::Output, CmdError> {
    let mut command = Command::new("swift");
    command.arg("package");
    command
        .arg("--package-path")
        .arg(package)
        .arg("--scratch-path")
        .arg(resolved)
        // Package.resolved is the answer; a dependency it does not pin is a
        // refusal, not a newer resolution published as the release's input.
        .arg("--disable-automatic-resolution")
        .arg("resolve");
    crate::wait::output(&mut command).map_err(|error| {
        CmdError::click(format!(
            "cannot run swift package resolve for {}: {error}; no input was published",
            package.display()
        ))
    })
}

/// Resolve `package` as its committed `Package.resolved` pins it, into a
/// stage under `scratch`, and pack that resolution as a gzip-compressed tar
/// with normalized headers and entries starting at `.build/`.
pub(super) fn export(package: &Path, scratch: &Path) -> Result<tempfile::NamedTempFile, CmdError> {
    std::fs::create_dir_all(scratch)?;
    sweep_abandoned(scratch)?;
    let owned = format!("{STAGE_PREFIX}{}-", std::process::id());
    let stage = tempfile::Builder::new()
        .prefix(&owned)
        .tempdir_in(scratch)?;
    let resolved = stage.path().join(SCRATCH);
    let output = resolve(package, &resolved)?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let way_out = if stderr.contains(NESTED_SANDBOX_CAUSE) {
            NESTED_SANDBOX_WAY_OUT
        } else {
            ""
        };
        return Err(CmdError::click(format!(
            "swift package resolve in {} failed ({}): {}{}{way_out}; no input was published",
            package.display(),
            output.status,
            String::from_utf8_lossy(&output.stdout),
            stderr
        )));
    }
    stado_product::swift_cache::make_portable(&resolved).map_err(|error| {
        CmdError::click(format!(
            "cannot make SwiftPM artifact paths portable: {error:#}; no input was published"
        ))
    })?;
    let archive = tempfile::Builder::new()
        .prefix(&owned)
        .tempfile_in(scratch)?;
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

/// Emit every entry under `root/relative` in name order.
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
