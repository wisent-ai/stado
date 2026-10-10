//! The lockfile half of `stado quality check`.
//!
//! A source install builds a Cargo product with `cargo build --locked`, which
//! refuses a `Cargo.lock` that does not already resolve the manifest beside it.
//! That refusal used to be the first place a stale lock showed up, after the
//! build had started. `cargo metadata --locked` asks the same question by
//! resolving the dependency graph without compiling anything, so the check can
//! answer it before a build is handed to anyone.
//!
//! Before the lock is asked about, the workspace's own manifests are read
//! (`cargo metadata --no-deps`) for a path dependency that leaves the exported
//! tree. The tree is one repository's commit, exported alone: a sibling
//! checkout the manifest reaches with `path = "../<repo>/…"` is not in it, on
//! this check or on a release worker, and Cargo's own answer to that is a
//! missing-manifest error that reads as a lockfile problem. It is not one, and
//! no lock rewrite creates the missing source, so the dependency is refused by
//! its own name, with the manifest that declares it and the two declarations
//! that do carry a sibling into a release.

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

/// Every path dependency of the workspace at `manifest` whose declared path
/// is not inside `tree`, as (declaring package, dependency name, path), read
/// from the workspace's own manifests without resolving the graph. A path
/// that does not exist is outside: the tree holds nothing at it.
fn path_dependencies_outside(
    workspace: &Path,
    tree: &Path,
    shown: &Path,
    revision: &str,
) -> Result<Vec<(String, String, PathBuf)>, CmdError> {
    let output = crate::wait::output(
        Command::new(super::installed("cargo")?)
            .args([
                "metadata",
                "--no-deps",
                "--offline",
                "--format-version",
                "1",
                "--manifest-path",
                "Cargo.toml",
            ])
            .current_dir(workspace)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped()),
    )
    .map_err(|error| {
        CmdError::from(error).within(format!(
            "cannot run cargo metadata --no-deps --format-version 1 --manifest-path Cargo.toml \
             in {} for {} at {revision}",
            workspace.display(),
            shown.display()
        ))
    })?;
    if !output.status.success() {
        return Err(CmdError::refused(format!(
            "stado quality check: cargo metadata --no-deps --format-version 1 --manifest-path \
             Cargo.toml failed with {} in {} for {} at {revision}: {}",
            output.status,
            workspace.display(),
            shown.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    let metadata: serde_json::Value = serde_json::from_slice(&output.stdout).map_err(|error| {
        CmdError::refused(format!(
            "stado quality check: cargo metadata --no-deps for {} at {revision} is not the \
             JSON this build reads: {error}",
            shown.display()
        ))
    })?;
    // The exported tree may be reached through a symlinked parent (the build
    // area under Stado's home); Cargo reports the path as it resolved it.
    let tree = tree.canonicalize().map_err(|error| {
        CmdError::click(format!(
            "cannot resolve the exported tree {}: {error}",
            tree.display()
        ))
        .stating(crate::cli::entry::error::io_failure_code(error.kind()))
    })?;
    let mut outside = Vec::new();
    for package in metadata["packages"].as_array().into_iter().flatten() {
        let name = package["name"].as_str().ok_or_else(|| {
            CmdError::refused(format!(
                "stado quality check: cargo metadata --no-deps for {} at {revision} lists a \
                 package without a name",
                shown.display()
            ))
        })?;
        for dependency in package["dependencies"].as_array().into_iter().flatten() {
            let Some(path) = dependency["path"].as_str() else {
                continue;
            };
            let path = PathBuf::from(path);
            let inside = match path.canonicalize() {
                Ok(resolved) => resolved.starts_with(&tree),
                Err(_absent) => false,
            };
            if !inside {
                let dependency = dependency["name"].as_str().ok_or_else(|| {
                    CmdError::refused(format!(
                        "stado quality check: cargo metadata --no-deps for {} at {revision} \
                         lists a dependency of package {name} without a name",
                        shown.display()
                    ))
                })?;
                outside.push((name.to_string(), dependency.to_string(), path));
            }
        }
    }
    Ok(outside)
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
        let outside = path_dependencies_outside(&workspace, tree, &shown, revision)?;
        if let Some(((package, dependency, path), rest)) = outside.split_first() {
            let others: Vec<&str> = rest.iter().map(|(_, name, _)| name.as_str()).collect();
            let also = if others.is_empty() {
                String::new()
            } else {
                format!(" (also outside: {})", others.join(", "))
            };
            return Err(CmdError::refused(format!(
                "stado quality check: {} (package {package}) declares dependency {dependency} \
                 at path {}, which is outside the committed tree of {revision}{also}: a \
                 source install and a release build carry only this repository's commit, so \
                 that path is absent there and no Cargo.lock change creates it. Declare it as \
                 a git dependency on its canonical origin - `stado product cargo` resolves it \
                 to the canonical checkout here and to the release's declared private source \
                 input on a worker - or pin the sibling as a release input in \
                 .wisent-release.json (inputs.<name>) and reach it there",
                shown.display(),
                path.display(),
            )));
        }
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
