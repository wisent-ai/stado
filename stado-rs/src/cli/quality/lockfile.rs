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

/// Refuse the first workspace in `tree` whose committed lock does not resolve
/// its manifest, naming the manifest (relative to the checkout) and what cargo
/// said; the tree is resolved, never compiled. Cargo runs from the workspace,
/// because it reads `.cargo/config.toml` from its working directory, not from
/// `--manifest-path` (a product's git-fetch-with-cli for private sources).
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
            &mut Command::new(super::installed("cargo")?)
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
        .map_err(|error| CmdError::from(error).within("cannot run cargo metadata"))?;
        if !output.status.success() {
            return Err(CmdError::refused(format!(
                "stado quality check: the Cargo.lock beside {} at {revision} does not resolve its \
                 manifest, so the install's cargo build --locked would refuse it: {}; resolve it \
                 with `cargo tree --depth 0` in that directory and commit Cargo.lock",
                shown.display(),
                String::from_utf8_lossy(&output.stderr).trim()
            )));
        }
    }
    Ok(())
}
