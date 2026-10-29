//! The refusal of a source commit that no canonical branch carries.
//!
//! Only a commit `origin/main` or the checkout's own `main` carries is
//! installed. A commit outside both is the request refusing itself: no
//! retry of the same request changes it, and the caller is told so with the
//! failure attributed to the request, never as an unknown failure.

use std::path::{Path, PathBuf};

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
