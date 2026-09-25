use super::super::{ownership, Placement};
use crate::state::ProductState;
use anyhow::Result;
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
};

pub fn observed(
    current: &ProductState,
    restored: &BTreeMap<PathBuf, Value>,
    shared: &BTreeSet<PathBuf>,
) -> Result<ProductState> {
    let mut previous = match &current.previous {
        Some(previous) => (**previous).clone(),
        None => {
            let mut absent = current.clone();
            absent.status = "absent".to_owned();
            absent.installed_paths.clear();
            absent.backups.clear();
            absent.previous = None;
            absent.release = None;
            absent.source_revision = None;
            absent.source_directory = None;
            absent.extra.clear();
            return Ok(absent);
        }
    };
    if previous.status == "absent" {
        return Ok(previous);
    }
    let mut observed = serde_json::Map::new();
    let mut paths = Vec::new();
    let mut absent = Vec::new();
    for path in &previous.installed_paths {
        let fingerprint = if let Some(fingerprint) = restored.get(path) {
            Some(fingerprint.clone())
        } else if ownership::overlaps(path, shared) {
            match path.symlink_metadata() {
                Ok(_) => Some(
                    Placement {
                        source: path.clone(),
                        destination: path.clone(),
                        symbolic: false,
                    }
                    .fingerprint()?,
                ),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                Err(error) => return Err(error.into()),
            }
        } else {
            None
        };
        if let Some(fingerprint) = fingerprint {
            observed.insert(path.to_string_lossy().into_owned(), fingerprint);
            paths.push(path.clone());
        } else {
            absent.push(path.clone());
        }
    }
    let observed = Value::Object(observed);
    if previous.extra.get("placement_fingerprints") != Some(&observed) {
        previous.extra.insert("rollback_unverified_source".to_owned(), json!({
            "recorded_source_revision": previous.source_revision.take(), "recorded_release": previous.release.take(),
            "absent_paths": absent,
            "reason": "retained bytes were not bound to the predecessor's recorded source; rollback restores bytes without claiming that source"}));
    }
    previous.installed_paths = paths;
    previous
        .extra
        .insert("placement_fingerprints".to_owned(), observed);
    Ok(previous)
}
