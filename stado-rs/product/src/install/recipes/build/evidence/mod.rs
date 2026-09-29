//! Where a source install keeps its evidence, and how much of it is kept.
//!
//! Each install writes `<checkout>/.wisent-output/install/<uuid>` holding a
//! committed-source export and that export's whole build output, which for a
//! large product is over a gigabyte. A checkout keeps the newest few; an
//! install that failed keeps only its logs and receipt.

pub(super) mod failures;

use anyhow::{Context, Result};
use std::{
    fs,
    path::{Path, PathBuf},
    time::SystemTime,
};

/// How many earlier evidence trees of one checkout are kept.
const KEPT: usize = 3;

/// A new evidence directory under `<root>/.wisent-output/install`, after
/// removing all but the newest [`KEPT`] earlier ones. An install running in
/// another process is among the newest, so its tree is kept.
pub(super) fn directory(root: &Path) -> Result<PathBuf> {
    let parent = root.join(".wisent-output/install");
    if let Ok(entries) = fs::read_dir(&parent) {
        let mut earlier: Vec<(SystemTime, PathBuf)> = entries
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
            .filter_map(|entry| Some((entry.metadata().ok()?.modified().ok()?, entry.path())))
            .collect();
        earlier.sort_by(|left, right| right.0.cmp(&left.0));
        for (_, stale) in earlier.into_iter().skip(KEPT) {
            fs::remove_dir_all(&stale).with_context(|| {
                format!("removing earlier install evidence {}", stale.display())
            })?;
        }
    }
    Ok(parent.join(uuid::Uuid::new_v4().to_string()))
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
