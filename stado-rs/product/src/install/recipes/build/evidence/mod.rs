//! Where a source install keeps its evidence, and what it keeps.
//!
//! Each install writes `<checkout area>/install/<run>`
//! ([`runs::checkout_area`], under Stado's home, never in the checkout)
//! holding a committed-source export and that export's whole build output,
//! which for a large product is over a gigabyte. A build whose build step
//! fails drops them; a build that passed is retained ([`retained`]) until an
//! installation of it succeeds, which sheds it (`Prepared::scratch`). The run
//! keeps its files — logs, receipt and its measured size, which the next
//! build's free-space check reads ([`runs::fresh_build`]).
//!
//! An install builds; it formats, lints and checks nothing. Whether the code
//! compiles is learned from its tests and from reading it, and a revision
//! that does not build fails its build step naming the evidence.

pub(super) mod retained;

use crate::common::runs::{self, Run};
use anyhow::{Context, Result};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{fs, path::Path};

/// Installs record no quality failures any more; the records an earlier
/// Stado kept in the build area `area` are read by nothing, so the next
/// install of the checkout removes them.
pub(super) fn shed_retired(area: &Path, id: &str) -> Result<()> {
    let retired = area.join("install-failures");
    match fs::remove_dir_all(&retired) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| {
            format!(
                "{id}: the retired install-failure records {} cannot be removed",
                retired.display()
            )
        }),
    }
}

/// The product, platform, revision and catalog recipe an install builds: a
/// different recipe is a different install even at one revision.
pub(super) fn key(id: &str, platform: &str, revision: &str, recipe: &Value) -> String {
    let recipe = recipe.to_string();
    let mut digest = Sha256::new();
    for part in [id, platform, revision, recipe.as_str()] {
        digest.update(part.as_bytes());
        digest.update([0]);
    }
    hex::encode(digest.finalize())
}

/// A new install run under the build area of the checkout `root`, held in
/// use until the returned value is dropped, and refused when the volume
/// cannot hold another run the size of the last one.
pub(super) fn directory(home: &Path, root: &Path) -> Result<Run> {
    runs::fresh_build(
        &runs::checkout_area(home, root)?.join("install"),
        &uuid::Uuid::new_v4().to_string(),
    )
}

/// A source build in progress. Unless it is marked finished, leaving scope
/// drops the export, its build output and the materialised inputs, which
/// nothing reads again after a failure, and keeps the evidence directory's
/// logs and receipt — unless the run was kept ([`runs::keep`]) because its
/// build step passed, which the next install resumes. A removal that fails
/// leaves the tree to the retention above rather than hiding the install's
/// own error.
pub(super) struct Build<'a> {
    evidence: &'a Path,
    finished: bool,
}

impl<'a> Build<'a> {
    pub(super) fn start(evidence: &'a Path) -> Self {
        Self {
            evidence,
            finished: false,
        }
    }

    pub(super) fn finished(mut self) {
        self.finished = true;
    }
}

impl Drop for Build<'_> {
    fn drop(&mut self) {
        if self.finished || runs::kept(self.evidence) {
            return;
        }
        let _ = runs::shed(self.evidence);
    }
}
