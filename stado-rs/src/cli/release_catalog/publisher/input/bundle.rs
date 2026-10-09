//! Export exactly the requested commit and its reachable history, without
//! changing a checkout or publishing unrelated branches and tags.

use std::path::Path;
use std::process::Command;

use crate::cli::CmdError;

use super::git;

pub(super) fn export(
    source: &Path,
    commit: &str,
    scratch: &Path,
) -> Result<tempfile::NamedTempFile, CmdError> {
    std::fs::create_dir_all(scratch)?;
    let archive = tempfile::NamedTempFile::new_in(scratch)?;
    let metadata = tempfile::tempdir_in(scratch)?;
    let objects = text(
        source,
        &[
            "rev-parse",
            "--path-format=absolute",
            "--git-path",
            "objects",
        ],
    )?;
    let object_format = text(source, &["rev-parse", "--show-object-format"])?;
    git(
        source,
        &[
            "init",
            "--bare",
            "--object-format",
            object_format.trim(),
            &metadata.path().to_string_lossy(),
        ],
    )?;
    // Only Git metadata is isolated: objects are read from the canonical
    // repository, without a clone or checkout. npm requires an advertised
    // HEAD even when its dependency pins an exact commit.
    isolated_git(
        metadata.path(),
        objects.trim(),
        &["update-ref", "--no-deref", "HEAD", commit],
    )?;
    isolated_git(
        metadata.path(),
        objects.trim(),
        &[
            "bundle",
            "create",
            &archive.path().to_string_lossy(),
            "HEAD",
        ],
    )?;
    metadata.close().map_err(|error| {
        CmdError::click(format!(
            "cannot remove Git bundle metadata under {}: {error}; no input was published",
            scratch.display()
        ))
    })?;
    Ok(archive)
}

fn text(source: &Path, arguments: &[&str]) -> Result<String, CmdError> {
    String::from_utf8(git(source, arguments)?).map_err(|error| {
        CmdError::click(format!(
            "git {} returned invalid UTF-8: {error}",
            arguments.join(" ")
        ))
    })
}

fn isolated_git(metadata: &Path, objects: &str, arguments: &[&str]) -> Result<(), CmdError> {
    let output = crate::wait::output(
        Command::new("git")
            .arg("--git-dir")
            .arg(metadata)
            .args(arguments)
            .env("GIT_OBJECT_DIRECTORY", objects),
    )
    .map_err(|error| {
        CmdError::click(format!(
            "cannot run git {} in {}: {error}",
            arguments.join(" "),
            metadata.display()
        ))
    })?;
    if !output.status.success() {
        return Err(CmdError::click(format!(
            "git {} in {} failed ({}): {}{}",
            arguments.join(" "),
            metadata.display(),
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )));
    }
    Ok(())
}
