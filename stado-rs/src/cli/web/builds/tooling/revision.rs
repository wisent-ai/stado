//! The commit the artifact was cut from, recorded beside it.

use std::path::Path;
use std::process::{Command, Stdio};

use crate::cli::web::builds::tooling::command::exit_report;
use crate::cli::CmdError;

/// The commit the artifact was cut from, recorded beside it.
///
/// The release worker unpacks a source *snapshot*, not a clone, so there is no
/// `.git` in it to ask - `git rev-parse HEAD` answers `fatal: not a git
/// repository` and exit 128 after a successful `next build`, which is the
/// worst possible moment to discover it. The revision is part of the release
/// contract instead: `stado release submit` passes `WISENT_SOURCE_COMMIT`
/// beside `WISENT_VERSION` and `WISENT_SOURCE_SHA256`, and that is the commit
/// the release is bound to rather than whatever HEAD happens to be.
///
/// A checkout is still the case for `stado web build` run by hand outside a
/// release, so git is still consulted and the failure of both is one
/// sentence naming both.
pub(in crate::cli::web::builds) fn revision(source: &Path) -> Result<String, CmdError> {
    if let Ok(declared) = std::env::var("WISENT_SOURCE_COMMIT") {
        let declared = declared.trim();
        if !declared.is_empty() {
            return Ok(declared.to_string());
        }
    }
    let output = Command::new("git")
        .arg("-C")
        .arg(source)
        .args(["rev-parse", "HEAD"])
        .stdin(Stdio::null())
        .output()
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                CmdError::click(
                    "WISENT_SOURCE_COMMIT is unset and `git` is not on PATH: the builder cannot record which commit the artifact was cut from",
                )
            } else {
                CmdError::click(format!("cannot run `git rev-parse HEAD`: {error}"))
            }
        })?;
    if !output.status.success() {
        return Err(CmdError::click(format!(
            "WISENT_SOURCE_COMMIT is unset and `git rev-parse HEAD` in {} failed with {}: {}",
            source.display(),
            exit_report(output.status),
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    let revision = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if revision.is_empty() {
        return Err(CmdError::click(format!(
            "`git rev-parse HEAD` in {} printed nothing",
            source.display()
        )));
    }
    Ok(revision)
}
