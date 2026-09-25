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
    if let Some(receipt) = &state.release {
        super::super::release::verify_files(receipt)?;
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
