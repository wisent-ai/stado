//! The branch a canonical checkout works on, and the refusal of a source
//! commit that no canonical branch carries.
//!
//! Only a commit `origin/main` or the checkout's own `main` carries is
//! installed. A commit outside both is the request refusing itself: no
//! retry of the same request changes it, and the caller is told so with the
//! failure attributed to the request, never as an unknown failure.

use super::{git, repository};
use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};

/// Make sure `root` holds `revision`: a commit pushed after the checkout's
/// last fetch is fetched from its origin first, and one origin does not
/// carry either is refused naming the checkout, instead of a read at it that
/// answers "names no commit" or "exists on disk, but not in <revision>".
pub fn hold(root: &Path, revision: &str) -> Result<()> {
    let commit = format!("{revision}^{{commit}}");
    let holds = || git(root, &["cat-file", "-e", &commit]).is_ok();
    if holds() {
        return Ok(());
    }
    git(root, &["fetch", "--quiet", "origin"])
        .with_context(|| format!("fetching {}'s origin to read {revision}", root.display()))?;
    if !holds() {
        bail!(
            "{revision} is not a commit of {} even after fetching its origin",
            root.display()
        );
    }
    Ok(())
}

/// The branch a canonical checkout works on: `main` for a repository of
/// Wisent's organization, whose one checkout works on main, and for another's
/// (a Swift package a desktop app uses) the default branch its origin names
/// (`origin/HEAD`, which a clone sets), which is not always main. A checkout
/// with no GitHub origin is ours and works on main.
pub(super) fn canonical_branch(root: &Path) -> Result<String> {
    let ours = format!("{}/", crate::documentation::WISENT_ORGANIZATION);
    let origin = git(root, &["config", "--get", "remote.origin.url"]).ok();
    match origin.as_deref().and_then(repository) {
        Some(foreign) if !foreign.starts_with(&ours) => {
            let head = git(root, &["symbolic-ref", "--short", "refs/remotes/origin/HEAD"])
                .with_context(|| {
                    format!(
                        "{} ({foreign}) names no default branch of its origin; run `git -C {} remote set-head origin --auto`",
                        root.display(),
                        root.display()
                    )
                })?;
            Ok(head.trim_start_matches("origin/").to_owned())
        }
        _ => Ok("main".to_owned()),
    }
}

#[derive(Debug)]
pub struct NotCanonical {
    pub commit: String,
    pub checkout: PathBuf,
}

impl NotCanonical {
    pub(super) fn new(commit: &str, checkout: &Path) -> Self {
        Self {
            commit: commit.to_owned(),
            checkout: checkout.to_path_buf(),
        }
    }
}

impl std::fmt::Display for NotCanonical {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "neither origin/main nor {}'s main carries {}; only canonical source is installed. \
             Merge it into main (and push it to origin/main) and install again, or name a \
             commit main carries with --source-commit",
            self.checkout.display(),
            self.commit
        )
    }
}

impl std::error::Error for NotCanonical {}
