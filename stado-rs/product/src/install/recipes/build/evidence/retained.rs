//! A build that passed its quality gates and its build step is kept, per
//! product, platform, revision and catalog recipe (the
//! [`super::failures::key`]), until an installation of it succeeds.
//!
//! Signing, staging, placement and activation come after the build, and each
//! can fail for a reason outside the tree: a vault that cannot be reached, a
//! keychain that does not hold the identity, a unit that does not start. An
//! install repeated after such a failure resumes at the step that failed with
//! the retained tree instead of exporting, checking and compiling the same
//! revision again. A successful installation sheds the tree
//! ([`crate::install::settle`]); the record that then names a shed tree is
//! removed the next time it is read.

use anyhow::{Context, Result};
use serde_json::{json, Value};
use std::{
    fs,
    path::{Path, PathBuf},
};

use crate::common::atomic_json;

/// Where a checkout's retained builds are recorded: its build area under
/// Stado's home, beside the quality failures.
fn record_path(area: &Path, key: &str) -> PathBuf {
    area.join("install-built").join(format!("{key}.json"))
}

/// Record that `evidence` holds a build of `revision` whose quality gates and
/// build step passed.
pub(in crate::install::recipes::build) fn keep(
    area: &Path,
    key: &str,
    evidence: &Path,
    product: &str,
    revision: &str,
) -> Result<()> {
    let path = record_path(area, key);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("creating the retained-build record {}", parent.display()))?;
    }
    atomic_json(
        &path,
        &json!({
            "product": product,
            "revision": revision,
            "evidence": evidence,
            "built_at": crate::common::now(),
        }),
    )
}

/// The evidence directory of the retained build for `key`, when one is
/// recorded and its tree is still there.
pub(in crate::install::recipes::build) fn find(area: &Path, key: &str) -> Result<Option<PathBuf>> {
    let path = record_path(area, key);
    let body = match fs::read(&path) {
        Ok(body) => body,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error)
                .with_context(|| format!("reading the retained-build record {}", path.display()))
        }
    };
    let record: Value = serde_json::from_slice(&body)
        .with_context(|| format!("parsing the retained-build record {}", path.display()))?;
    let evidence = PathBuf::from(
        record["evidence"]
            .as_str()
            .with_context(|| format!("{} names no evidence directory", path.display()))?,
    );
    if evidence.join("source").is_dir() {
        return Ok(Some(evidence));
    }
    // The tree was shed by a successful installation, or removed by hand:
    // nothing is retained for this key any more.
    fs::remove_file(&path).with_context(|| {
        format!(
            "removing the spent retained-build record {}",
            path.display()
        )
    })?;
    Ok(None)
}
