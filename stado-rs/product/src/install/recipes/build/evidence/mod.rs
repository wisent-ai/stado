//! Where a source install keeps its evidence, and what a failed one keeps.
//!
//! Each install writes `<checkout>/.wisent-output/install/<run>` holding a
//! committed-source export and that export's whole build output, which for a
//! large product is over a gigabyte. The checkout keeps the newest few runs
//! ([`runs::KEPT_BUILDS`]); an install that failed keeps only its logs and
//! receipt.

pub(super) mod failures;

use crate::common::runs::{self, Run};
use anyhow::Result;
use std::{fs, path::Path};

/// A new install run under `<root>/.wisent-output/install`, held in use until
/// the returned value is dropped, and refused when the volume cannot hold
/// another run the size of the last one.
pub(super) fn directory(root: &Path) -> Result<Run> {
    runs::fresh_build(
        &root.join(".wisent-output/install"),
        &uuid::Uuid::new_v4().to_string(),
    )
}

/// A source build in progress. Unless it is marked finished, leaving scope
/// drops the export, its build output and the materialised inputs, which
/// nothing reads again after a failure, and keeps the evidence directory's
/// logs and receipt. A removal that fails leaves the tree to the retention
/// above rather than hiding the install's own error.
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
        if self.finished {
            return;
        }
        for part in ["source", "inputs"] {
            let path = self.evidence.join(part);
            if path.exists() {
                let _ = fs::remove_dir_all(&path);
            }
        }
    }
}
