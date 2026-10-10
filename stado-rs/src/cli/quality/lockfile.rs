//! The lockfile half of `stado quality check`.
//!
//! A source install builds a Cargo product with `cargo build --locked`, which
//! refuses a `Cargo.lock` that does not already resolve the manifest beside it.
//! That refusal used to be the first place a stale lock showed up, after the
//! build had started. `cargo metadata --locked` asks the same question by
//! resolving the dependency graph without compiling anything, so the check can
//! answer it before a build is handed to anyone.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::cli::CmdError;

/// The Cargo workspaces an install of `tree` could build: a `Cargo.toml` with
/// a `Cargo.lock` beside it, at the root or one directory down (`<product>-rs`
/// layouts), in path order.
fn workspaces(tree: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut consider = |dir: &Path| {
        if dir.join("Cargo.toml").is_file() && dir.join("Cargo.lock").is_file() {
            found.push(dir.to_path_buf());
        }
    };
    consider(tree);
    if let Ok(entries) = std::fs::read_dir(tree) {
        let mut children: Vec<PathBuf> = entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.is_dir())
            .collect();
        children.sort();
        for child in children {
            consider(&child);
        }
    }
    found
}

/// Resolve each committed workspace without compiling it. A refusal names the
/// operation, exit status, exported workspace, original manifest and revision,
/// followed by Cargo's cause; missing source, transport and lock failures must
/// not all be diagnosed as stale lockfiles. Cargo reads `.cargo/config.toml`
/// from its working directory, not from `--manifest-path`.
/// Progress goes where the caller's `report` sends it: a check whose stdout
/// is a JSON answer (`stado release changes submit --json`) reports on stderr.
pub(super) fn check(
    tree: &Path,
    checkout: &Path,
    revision: &str,
    report: super::Report,
) -> Result<(), CmdError> {
    for workspace in workspaces(tree) {
        let manifest = workspace.join("Cargo.toml");
        let relative = manifest.strip_prefix(tree).map_err(|error| {
            CmdError::click(format!(
                "stado quality check: workspace manifest {} is not inside the checked tree {}: \
                 {error}",
                manifest.display(),
                tree.display()
            ))
        })?;
        let shown = checkout.join(relative);
        report.say(&format!(
            "stado quality check: cargo metadata --locked for {}",
            shown.display()
        ));
        let output = crate::wait::output(
            Command::new(super::installed("cargo")?)
                .args([
                    "metadata",
                    "--locked",
                    "--format-version",
                    "1",
                    "--manifest-path",
                    "Cargo.toml",
                ])
                .current_dir(&workspace)
                .stdout(Stdio::null())
                .stderr(Stdio::piped()),
        )
        .map_err(|error| {
            CmdError::from(error).within(format!(
                "cannot run cargo metadata --locked --format-version 1 \
                 --manifest-path Cargo.toml in {} for {} at {revision}",
                workspace.display(),
                shown.display()
            ))
        })?;
        if !output.status.success() {
            return Err(CmdError::refused(format!(
                "stado quality check: cargo metadata --locked --format-version 1 \
                 --manifest-path Cargo.toml failed with {} in {} for {} at {revision}: {}",
                output.status,
                workspace.display(),
                shown.display(),
                String::from_utf8_lossy(&output.stderr).trim()
            )));
        }
    }
    Ok(())
}
