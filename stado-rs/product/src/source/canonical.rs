//! The branch a canonical checkout works on, and the refusal of a source
//! commit that no canonical branch carries.
//!
//! Only a commit `origin/main` or the checkout's own `main` carries is
//! installed. A commit outside both is the request refusing itself: no
//! retry of the same request changes it, and the caller is told so with the
//! failure attributed to the request, never as an unknown failure.

use super::git;
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

/// The branch a canonical checkout works on: the default branch its origin
/// names. A clone records it (`origin/HEAD`); a checkout that never did (one
/// made by `git init` and a remote) asks its origin, which names it in the
/// symbolic `HEAD` it advertises. A recorded `origin/HEAD` is read only while
/// the branch it names is still a remote-tracking branch of the checkout: a
/// clone keeps the default it saw when it was made, and once that branch is
/// deleted on the origin (and pruned here) the recorded name is a dangling
/// symbolic ref naming a branch nobody works on, so the origin is asked
/// instead. No organization or repository is told apart by name, and nothing
/// is written into the checkout.
pub(super) fn canonical_branch(root: &Path) -> Result<String> {
    if let Ok(head) = git(
        root,
        &["symbolic-ref", "--short", "refs/remotes/origin/HEAD"],
    ) {
        let tracking = format!("refs/remotes/{head}");
        if git(root, &["rev-parse", "--verify", "--quiet", &tracking]).is_ok() {
            return Ok(head.trim_start_matches("origin/").to_owned());
        }
    }
    let advertised =
        git(root, &["ls-remote", "--symref", "origin", "HEAD"]).with_context(|| {
            format!(
                "{} records no default branch of its origin, and its origin could not be asked",
                root.display()
            )
        })?;
    advertised
        .lines()
        .find_map(|line| {
            line.strip_prefix("ref: refs/heads/")?
                .strip_suffix("\tHEAD")
        })
        .map(str::to_owned)
        .with_context(|| {
            format!(
                "the origin of {} names no default branch: {advertised}",
                root.display()
            )
        })
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
