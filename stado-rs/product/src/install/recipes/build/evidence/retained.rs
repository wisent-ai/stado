//! A build that passed its build step is kept, per product, platform,
//! revision and catalog recipe (the [`super::key`]), until an installation of
//! it succeeds.
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
/// Stado's home.
pub(in crate::install::recipes::build) fn record_path(area: &Path, key: &str) -> PathBuf {
    area.join("install-built").join(format!("{key}.json"))
}

/// The newest live retained build of `product` at `revision` in `area`,
/// under whatever key: what `stado product finish` takes over. The binary
/// that built the tree keyed it with its own catalog recipe, and the newer
/// binary finishing it may spell that recipe otherwise, so a finish names
/// the build by revision, not by key; the handing-off install keeps its
/// build and hands off at once under the writer lock, so the newest build of
/// the revision is the one it handed off. A record that cannot be read
/// refuses with its cause; one naming a shed tree is removed, as [`find`]
/// removes it.
pub(in crate::install::recipes::build) fn newest_of_revision(
    area: &Path,
    product: &str,
    revision: &str,
) -> Result<Option<PathBuf>> {
    let records = area.join("install-built");
    let entries = match fs::read_dir(&records) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error).with_context(|| format!("listing {}", records.display()))
        }
    };
    let mut newest: Option<(String, PathBuf)> = None;
    for entry in entries {
        let path = entry.with_context(|| format!("listing {}", records.display()))?.path();
        let record = read(&path)?;
        if record["product"] != product || record["revision"] != revision {
            continue;
        }
        let built_at = record["built_at"]
            .as_str()
            .with_context(|| format!("{} names no built_at time", path.display()))?
            .to_owned();
        if let Some(evidence) = live(&path, &record)? {
            if newest.as_ref().map_or(true, |(time, _)| built_at > *time) {
                newest = Some((built_at, evidence));
            }
        }
    }
    Ok(newest.map(|(_, evidence)| evidence))
}

/// The retained-build record at `path`.
fn read(path: &Path) -> Result<Value> {
    let body = fs::read(path)
        .with_context(|| format!("reading the retained-build record {}", path.display()))?;
    serde_json::from_slice(&body)
        .with_context(|| format!("parsing the retained-build record {}", path.display()))
}

/// The evidence directory `record` (read from `path`) names, when its tree
/// is still there; a record naming a shed tree is removed.
fn live(path: &Path, record: &Value) -> Result<Option<PathBuf>> {
    let evidence = PathBuf::from(
        record["evidence"]
            .as_str()
            .with_context(|| format!("{} names no evidence directory", path.display()))?,
    );
    if evidence.join("source").is_dir() {
        return Ok(Some(evidence));
    }
    // The tree was shed by a successful installation, or removed by hand:
    // nothing is retained under this record any more.
    fs::remove_file(path).with_context(|| {
        format!(
            "removing the spent retained-build record {}",
            path.display()
        )
    })?;
    Ok(None)
}

/// Record that `evidence` holds a build of `revision` whose build step
/// passed.
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
    let recorded = path
        .try_exists()
        .with_context(|| format!("reading the retained-build record {}", path.display()))?;
    if !recorded {
        return Ok(None);
    }
    live(&path, &read(&path)?)
}
