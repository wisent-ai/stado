//! Whether a compiler source in SwiftPM's build tree is a dependency's.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

/// The git repository inside `scratch` that holds `source`, when there is
/// one: SwiftPM keeps its dependency clones in its build tree, and a source
/// in one of them is a dependency's, not a source this build generated. Git
/// answers which repository holds the source's directory; one whose git
/// directory lies inside the build tree is such a clone, and a directory in
/// no repository, or in one outside the build tree, holds generated output.
pub(super) fn repository_inside(source: &Path, scratch: &Path) -> Result<Option<PathBuf>> {
    let directory = source
        .parent()
        .with_context(|| format!("{} has no parent directory", source.display()))?;
    let answer = crate::common::capture(
        Command::new("git")
            .arg("-C")
            .arg(directory)
            .args(["rev-parse", "--absolute-git-dir"]),
    )?;
    if !answer.status.success() {
        return Ok(None);
    }
    let repository = PathBuf::from(String::from_utf8(answer.stdout)?.trim());
    Ok(repository.starts_with(scratch).then_some(repository))
}
