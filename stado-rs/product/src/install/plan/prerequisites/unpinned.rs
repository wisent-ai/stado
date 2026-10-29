//! The source revision an installation that names no coordinate builds.
//!
//! The canonical checkout's head is not it: a checkout other work is in the
//! middle of stands wherever that work left it, so a product installed as a
//! dependency was rebuilt from a head published history had long passed,
//! over a newer installation of the same product. An unpinned build takes
//! the product's published history: its origin's `main`, fetched now without
//! a prompt, else the revision the delivery routine last qualified green.
//! It never replaces an installed revision with one of its ancestors: the
//! installed one is built again instead. What it chose, and why, is said.

use std::path::Path;
use std::process::Command;

use anyhow::{bail, Result};

use crate::{
    common::{capture, Runtime},
    source,
    state::ProductState,
};

/// The branch of the origin an unpinned build installs.
const PUBLISHED: &str = "refs/remotes/origin/main";
/// The suffix a recorded source revision carries when its tree was dirty.
const DIRTY: &str = "-dirty";

/// The revision an unpinned installation of `id` on `surface`, whose
/// canonical checkout is `root`, builds, with why.
pub(super) fn choose(
    runtime: &Runtime,
    id: &str,
    surface: &str,
    root: &Path,
) -> Result<(String, String)> {
    let (published, why) = match fetched(root) {
        Ok(()) => (
            source::git(
                root,
                &["rev-parse", "--verify", &format!("{PUBLISHED}^{{commit}}")],
            )?,
            "its origin's main, fetched now".to_owned(),
        ),
        Err(unfetched) => match super::green(runtime, id, surface) {
            Ok(qualified) => (
                qualified,
                format!(
                    "the revision the delivery routine last qualified green, since its origin \
                     could not be fetched ({unfetched:#})"
                ),
            ),
            Err(unqualified) => bail!(
                "{id} {surface} names no source coordinate and has no published revision to \
                 build: its origin could not be fetched ({unfetched:#}) and {unqualified:#}; \
                 name one with --source-commit"
            ),
        },
    };
    let installed = ProductState::load(runtime, id, surface)?
        .and_then(|state| state.source_revision)
        .map(|revision| revision.trim_end_matches(DIRTY).to_owned());
    match installed {
        Some(installed) if installed != published && ancestor(root, &published, &installed)? => {
            Ok((
                installed.clone(),
                format!(
                    "the installed {installed}, built again: {why} gives {published}, an ancestor \
                     of it, and an installation never replaces a revision with an older one"
                ),
            ))
        }
        _ => Ok((published, why)),
    }
}

/// Fetch the origin of `root` without ever asking for a credential.
fn fetched(root: &Path) -> Result<()> {
    let output = capture(
        Command::new("git")
            .args(["fetch", "--quiet", "origin"])
            .env("GIT_TERMINAL_PROMPT", "0")
            .current_dir(root),
    )?;
    if output.status.success() {
        return Ok(());
    }
    bail!(
        "git fetch origin in {} ended with {}: {}",
        root.display(),
        output.status,
        String::from_utf8_lossy(&output.stderr).trim()
    )
}

/// Whether `older` is an ancestor of `newer` in `root`. `git merge-base
/// --is-ancestor` ends 0 when it is, 1 when it is not, and 128 for a revision
/// the checkout does not hold (git-merge-base(1)), which is no ancestor of
/// anything this checkout can build.
fn ancestor(root: &Path, older: &str, newer: &str) -> Result<bool> {
    let output = capture(
        Command::new("git")
            .args(["merge-base", "--is-ancestor", older, newer])
            .current_dir(root),
    )?;
    Ok(output.status.success())
}
