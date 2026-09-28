use super::Placement;
use crate::{
    common::{lock, Runtime},
    signing, state,
};
use anyhow::{bail, Result};
use serde_json::{json, Value};
use std::{
    collections::BTreeSet,
    fs::{self, File},
    path::{Path, PathBuf},
};

pub fn writer(runtime: &Runtime) -> Result<File> {
    lock(&runtime.home.join(".stado/products/ownership.lock"))
}

pub fn shared(runtime: &Runtime, product: &str, surface: &str) -> Result<BTreeSet<PathBuf>> {
    let mut paths = BTreeSet::new();
    for state in state::all(runtime)? {
        if state.status == "absent" || (state.product == product && state.surface == surface) {
            continue;
        }
        for path in state.protected_paths() {
            match path.canonicalize() {
                Ok(physical) => {
                    paths.insert(physical);
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
            paths.insert(path.clone());
        }
    }
    Ok(paths)
}

/// Whether `other` is another surface of `product` installed from the same
/// recipe source — the same kind, repository and manifest — so the files it
/// places are the ones this install is about to place.
///
/// transcript-lake's cli and service surfaces both place
/// `~/.stado/bin/transcript-lake` from one stado-release manifest, and each
/// refused to replace it while the other owned it, so neither could ever be
/// updated (2026-09-28). Only a finished receipt qualifies: `installed`, or
/// `rolled_back` to the files it restored. `ready` is a readiness verdict
/// `stado product status` computes, never a stored status, and requiring it
/// made the rule match nothing.
pub fn sibling(other: &state::ProductState, product: &str, surface: &str, recipe: &Value) -> bool {
    let same =
        |key: &str| other.recipe.get(key).is_some() && other.recipe.get(key) == recipe.get(key);
    other.product == product
        && other.surface != surface
        && matches!(other.status.as_str(), "installed" | "rolled_back")
        && same("kind")
        && same("repository")
        && same("manifest")
}

/// Paths other surfaces own, split into those a sibling installed from the
/// same recipe owns (which this install may replace, re-recording the
/// sibling) and every other owned path (which it may not change).
pub fn shared_for_install(
    runtime: &Runtime,
    product: &str,
    surface: &str,
    recipe: &Value,
) -> Result<(BTreeSet<PathBuf>, Vec<state::ProductState>)> {
    let mut strict = BTreeSet::new();
    let mut siblings = Vec::new();
    for state in state::all(runtime)? {
        if state.status == "absent" || (state.product == product && state.surface == surface) {
            continue;
        }
        if sibling(&state, product, surface, recipe) {
            siblings.push(state);
            continue;
        }
        for path in state.protected_paths() {
            match path.canonicalize() {
                Ok(physical) => {
                    strict.insert(physical);
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
            strict.insert(path.clone());
        }
    }
    Ok((strict, siblings))
}

/// Whether a sibling surface records `path` among its installed files.
pub fn sibling_owns(siblings: &[state::ProductState], path: &Path) -> bool {
    siblings
        .iter()
        .any(|sibling| sibling.installed_paths.iter().any(|owned| owned == path))
}

/// A sibling surface installed from the same recipe owned some of the paths
/// this install just replaced: its receipt now names this install's source
/// revision and the new fingerprints of those paths, so its own verification
/// and its next update read what is on disk.
pub fn rerecord_siblings(
    runtime: &Runtime,
    siblings: &[state::ProductState],
    installed: &state::ProductState,
    placements: &[Placement],
) -> Result<()> {
    for sibling in siblings {
        let touched: Vec<&Placement> = placements
            .iter()
            .filter(|placement| sibling.installed_paths.contains(&placement.destination))
            .collect();
        if touched.is_empty() {
            continue;
        }
        let mut updated = sibling.clone();
        let mut fingerprints = updated
            .extra
            .get("placement_fingerprints")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        for placement in touched {
            fingerprints.insert(
                placement.destination.to_string_lossy().into_owned(),
                placement.fingerprint()?,
            );
        }
        updated.extra.insert(
            "placement_fingerprints".to_owned(),
            Value::Object(fingerprints),
        );
        updated.extra.insert(
            "replaced_by".to_owned(),
            json!({"surface": installed.surface, "source_revision": installed.source_revision, "at": crate::common::now()}),
        );
        updated.source_revision = installed.source_revision.clone();
        updated.save(runtime)?;
    }
    Ok(())
}

pub fn overlaps(path: &Path, shared: &BTreeSet<PathBuf>) -> bool {
    let physical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    shared.iter().any(|other| {
        path.starts_with(other)
            || other.starts_with(path)
            || physical.starts_with(other)
            || other.starts_with(&physical)
    })
}

pub fn guard(current: &state::ProductState, path: &Path) -> Result<Option<serde_json::Value>> {
    match path.symlink_metadata() {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
        Ok(_) => {}
    }
    let actual = super::Placement {
        source: path.to_path_buf(),
        destination: path.to_path_buf(),
        symbolic: false,
    }
    .fingerprint()?;
    let installed = current
        .extra
        .get("placement_fingerprints")
        .and_then(|values| values.get(path.to_string_lossy().as_ref()));
    let retained = current
        .backups
        .iter()
        .find(|saved| saved.path == path)
        .and_then(|saved| {
            current
                .extra
                .get("backup_fingerprints")
                .and_then(|values| values.get(saved.backup.to_string_lossy().as_ref()))
        });
    if installed != Some(&actual) && retained != Some(&actual) {
        bail!("refusing to replace content not matched by this installation or its retained backup: {}", path.display());
    }
    Ok(Some(actual))
}
pub fn verify(state: &state::ProductState) -> Result<()> {
    verify_content(state)?;
    for path in &state.installed_paths {
        let report = signing::inspect(path)?;
        if !signing::acceptable(&report) {
            bail!(
                "{} has unstable code identity: {}",
                path.display(),
                report["error"]
            );
        }
    }
    Ok(())
}

/// The recorded release and placement fingerprints hold, whatever the code
/// identity of the files.
///
/// A rollback restores the bytes the install replaced, and those are
/// whatever the host ran before, often a local build with no Developer ID.
/// Requiring a stable identity of them refused the rollback AFTER it had
/// placed the backup, on 2026-09-27, leaving Tama's receipt `rolling_back`
/// with install and remove both refusing; `code_identities` records them.
pub fn verify_content(state: &state::ProductState) -> Result<()> {
    if let Some(receipt) = &state.release {
        super::super::plan::release::verify_files(receipt)?;
    }
    if let Some(fingerprints) = state
        .extra
        .get("placement_fingerprints")
        .and_then(Value::as_object)
    {
        for (path, expected) in fingerprints {
            let path = Path::new(path);
            let actual = if expected.get("link").is_some() {
                json!({"link": fs::read_link(path)?})
            } else {
                Placement {
                    source: path.to_path_buf(),
                    destination: path.to_path_buf(),
                    symbolic: false,
                }
                .fingerprint()?
            };
            if &actual != expected {
                bail!(
                    "installed content differs from its prepared artifact: {}",
                    path.display()
                );
            }
        }
    }
    Ok(())
}

/// Each installed path's code identity, for a receipt that does not require
/// one to be stable.
pub fn code_identities(state: &state::ProductState) -> Result<Value> {
    let mut identities = serde_json::Map::new();
    for path in &state.installed_paths {
        identities.insert(path.display().to_string(), signing::inspect(path)?);
    }
    Ok(Value::Object(identities))
}
