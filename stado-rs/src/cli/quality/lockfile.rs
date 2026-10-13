//! The lockfile half of `stado quality check`.
//!
//! A source install builds a Cargo product with `cargo build --locked`, which
//! refuses a `Cargo.lock` that does not already resolve the manifest beside it.
//! That refusal used to be the first place a stale lock showed up, after the
//! build had started. Resolving the dependency graph under `--locked`
//! without compiling anything asks the same question, so the check can
//! answer it before a build is handed to anyone. Git dependencies are
//! fetched with the `git` program, so a private `wisent-ai` repository is
//! fetched with the credentials the checked push itself uses.
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

mod unpushed;

/// The Cargo workspaces an install of `tree` could build: a `Cargo.toml` with
/// a `Cargo.lock` beside it, at the root or one directory down (`<product>-rs`
/// layouts), in path order. A tree that cannot be listed is refused: read as
/// holding no workspace, every lock in it passed the check unasked.
fn workspaces(tree: &Path) -> Result<Vec<PathBuf>, CmdError> {
    let mut found = Vec::new();
    let mut consider = |dir: &Path| {
        if dir.join("Cargo.toml").is_file() && dir.join("Cargo.lock").is_file() {
            found.push(dir.to_path_buf());
        }
    };
    consider(tree);
    let unlisted =
        |error: std::io::Error| CmdError::click(format!("cannot list {}: {error}", tree.display()));
    let mut children = Vec::new();
    for entry in std::fs::read_dir(tree).map_err(unlisted)? {
        let path = entry.map_err(unlisted)?.path();
        if path.is_dir() {
            children.push(path);
        }
    }
    children.sort();
    for child in children {
        consider(&child);
    }
    Ok(found)
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

/// One Cargo command in the exported workspace. Git dependencies are fetched
/// with the `git` program (`net.git-fetch-with-cli`), which carries the
/// credentials the push being checked is itself made with; Cargo's built-in
/// transport carries none, so a private `wisent-ai` repository answered it
/// 'failed to authenticate' and the lock was never asked about.
fn cargo(
    workspace: &Path,
    shown: &Path,
    revision: &str,
    args: &[&str],
) -> Result<std::process::Output, CmdError> {
    crate::wait::output(
        Command::new(super::installed("cargo")?)
            .args(args)
            .args(["--manifest-path", "Cargo.toml"])
            .env("CARGO_NET_GIT_FETCH_WITH_CLI", "true")
            .current_dir(workspace)
            .stdout(Stdio::null())
            .stderr(Stdio::piped()),
    )
    .map_err(|error| {
        CmdError::from(error).within(format!(
            "cannot run cargo {} --manifest-path Cargo.toml in {} for {} at {revision}",
            args.join(" "),
            workspace.display(),
            shown.display()
        ))
    })
}

/// Resolve each committed workspace without compiling it, in two steps whose
/// failures mean different things. `cargo fetch` brings every source the
/// graph names into Cargo's cache; its failure is a source that could not be
/// fetched, and whether the lock resolves was never asked. `cargo metadata
/// --locked --offline` then asks the committed lock, put back byte for byte
/// after the fetch, and its refusal is a lock that does not resolve the
/// manifest. Each refusal names the command, exit status, exported
/// workspace, original manifest and revision, followed by Cargo's cause.
/// Progress goes where the caller's `report` sends it: a check whose stdout
/// is a JSON answer (`stado release changes submit --json`) reports on stderr.
pub(super) fn check(
    tree: &Path,
    checkout: &Path,
    revision: &str,
    report: super::Report,
) -> Result<(), CmdError> {
    for workspace in workspaces(tree)? {
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
            "stado quality check: cargo fetch, then cargo metadata --locked --offline for {}",
            shown.display()
        ));
        let lock = workspace.join("Cargo.lock");
        let committed = std::fs::read(&lock).map_err(|error| {
            CmdError::click(format!("cannot read {}: {error}", lock.display()))
                .stating(crate::cli::entry::error::io_failure_code(error.kind()))
        })?;
        let fetched = cargo(&workspace, &shown, revision, &["fetch"])?;
        // A fetch writes the entries a stale lock lacks; the lock asked about
        // below is the committed one.
        std::fs::write(&lock, &committed).map_err(|error| {
            CmdError::click(format!(
                "cannot put the committed {} back after cargo fetch: {error}",
                lock.display()
            ))
            .stating(crate::cli::entry::error::io_failure_code(error.kind()))
        })?;
        if !fetched.status.success() {
            // A pinned commit that only a sibling checkout holds is a push
            // missing on this machine, named before Cargo's own words.
            let unpushed = unpushed::unpushed(&committed, &workspace, checkout);
            if !unpushed.is_empty() {
                return Err(CmdError::refused(format!(
                    "stado quality check: {} at {revision} pins, in a manifest or its \
                     Cargo.lock, a git dependency commit its remote does not hold, so no other \
                     machine can build it: {}",
                    shown.display(),
                    unpushed.join("; ")
                )));
            }
            return Err(CmdError::click(format!(
                "stado quality check: cargo fetch --manifest-path Cargo.toml failed with {} in \
                 {} for {} at {revision}, so whether its Cargo.lock resolves was not asked: {}",
                fetched.status,
                workspace.display(),
                shown.display(),
                String::from_utf8_lossy(&fetched.stderr).trim()
            )));
        }
        let output = cargo(
            &workspace,
            &shown,
            revision,
            &["metadata", "--locked", "--offline", "--format-version", "1"],
        )?;
        if !output.status.success() {
            return Err(CmdError::refused(format!(
                "stado quality check: the committed Cargo.lock does not resolve its manifest: \
                 cargo metadata --locked --offline --format-version 1 --manifest-path \
                 Cargo.toml failed with {} in {} for {} at {revision}: {}",
                output.status,
                workspace.display(),
                shown.display(),
                String::from_utf8_lossy(&output.stderr).trim()
            )));
        }
    }
    Ok(())
}
