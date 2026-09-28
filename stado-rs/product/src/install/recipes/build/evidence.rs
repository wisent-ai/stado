//! Where a source install keeps its evidence, and how much of it is kept.
//!
//! Each install writes `<checkout>/.wisent-output/install/<uuid>` holding a
//! committed-source export and that export's whole build output, about
//! 1.2 GiB for Stado. Nothing removed them: on 2026-09-28 93 such trees
//! (116 GiB) filled lukasz-macbook's disk and every process on it failed its
//! writes.

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
