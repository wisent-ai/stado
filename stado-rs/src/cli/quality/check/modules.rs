//! The module half of `stado quality check`: every `mod name;` each Rust
//! crate of the exported tree reaches names a file the tree holds, and no
//! Swift package target holds two sources of one file name
//! ([`swift`]).
//!
//! A commit that deleted a module file another file still declares, or
//! declared one it never added, compiles nowhere, and until now the first to
//! learn it was the installation that built on it. The declarations are read
//! from the files alone — the same reading the version gate's reachability
//! check walks — so the commit is refused before anything compiles: on the
//! pre-push check, on a change hand-off, on a release submission and on the
//! delivery queue's gate step, each of which runs this check. The tree is
//! the commit's export, which holds tracked files only, so no build output or
//! dependency directory is walked.

mod swift;

use std::path::{Path, PathBuf};

use crate::cli::release_cmd::undeclared_files;
use crate::cli::CmdError;

/// Every directory under `directory` holding a `Cargo.toml` beside a `src/`.
/// A path that cannot be examined is refused naming it: skipped, the crate
/// behind it was never checked and the gate passed a tree it had not read.
fn crates(directory: &Path, into: &mut Vec<PathBuf>) -> Result<(), CmdError> {
    let manifest = directory.join("Cargo.toml");
    let sources = directory.join("src");
    if crate::primitives::presence::present(&manifest)?
        && manifest.is_file()
        && crate::primitives::presence::present(&sources)?
        && sources.is_dir()
    {
        into.push(directory.to_path_buf());
    }
    let entries = std::fs::read_dir(directory).map_err(|error| {
        CmdError::click(format!("cannot list {}: {error}", directory.display()))
    })?;
    for entry in entries {
        let entry = entry.map_err(|error| {
            CmdError::click(format!("cannot list {}: {error}", directory.display()))
        })?;
        let path = entry.path();
        // The entry itself, not what a link points at: a link is not walked.
        let kind = std::fs::symlink_metadata(&path).map_err(|error| {
            CmdError::click(format!("cannot examine {}: {error}", path.display()))
        })?;
        if kind.is_dir() {
            crates(&path, into)?;
        }
    }
    Ok(())
}

/// Refuse `revision` of `checkout`, exported at `tree`, when a crate in it
/// declares a module file the tree does not hold, or names through a
/// `crate::` path a module file the tree holds but no declaration reaches,
/// naming every such declaration and path.
pub(super) fn check(tree: &Path, checkout: &Path, revision: &str) -> Result<(), CmdError> {
    let mut found = Vec::new();
    crates(tree, &mut found)?;
    let (mut missing, mut unreached) = (Vec::new(), Vec::new());
    for crate_dir in &found {
        missing.extend(undeclared_files(crate_dir).map_err(CmdError::click)?);
        unreached.extend(super::paths::unreached(crate_dir).map_err(CmdError::click)?);
    }
    let colliding = swift::colliding(tree).map_err(CmdError::click)?;
    let exported = format!("{}/", tree.display());
    let listed = |lines: &[String]| {
        lines
            .iter()
            .map(|line| line.replace(&exported, ""))
            .collect::<Vec<_>>()
            .join("; ")
    };
    let mut refusals = Vec::new();
    if !missing.is_empty() {
        refusals.push(format!(
            "{revision} of {} declares module files it does not hold, so no build of it compiles: {}",
            checkout.display(),
            listed(&missing)
        ));
    }
    if !unreached.is_empty() {
        refusals.push(format!(
            "{revision} of {} uses modules no declaration reaches, so no build of it compiles: {}",
            checkout.display(),
            listed(&unreached)
        ));
    }
    if !colliding.is_empty() {
        refusals.push(format!(
            "{revision} of {} holds Swift sources of one file name in one Swift package target, \
             so SwiftPM has two producers of one object file and the target does not build: {}",
            checkout.display(),
            listed(&colliding)
        ));
    }
    if refusals.is_empty() {
        return Ok(());
    }
    Err(CmdError::refused(refusals.join(". ")))
}

/// The locks and the module declarations of `revision` of `checkout`, over
/// its committed tree exported beside the checkout's other quality runs, for
/// a product that declares no format gate: everything `stado quality check`
/// reads without running a gate, so without compiling.
pub(crate) async fn check_source(
    checkout: &Path,
    revision: &str,
    report: super::Report,
) -> Result<(), CmdError> {
    let runs = stado_product::common::runs::checkout_area(
        &crate::config_file::expand_tilde("~"),
        checkout,
    )
    .map_err(|error| CmdError::click(format!("{error:#}")))?
    .join("quality");
    let run_area = runs.join(format!("{}-{revision}-source", std::process::id()));
    let tree = run_area.join("source");
    stado_product::export_committed_source(checkout, revision, &tree)
        .map_err(|error| CmdError::unreachable(format!("cannot export {revision}: {error:#}")))?;
    let verdict = super::lockfile::check(&tree, checkout, revision, report)
        .and_then(|()| check(&tree, checkout, revision));
    std::fs::remove_dir_all(&run_area).map_err(|error| {
        CmdError::click(format!("cannot remove {}: {error}", run_area.display()))
    })?;
    verdict
}
