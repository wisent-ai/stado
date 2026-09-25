use crate::{
    common::{atomic_json, checked, relative, sha256, unpack, Runtime},
    source,
};
use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::{collections::BTreeMap, fs, path::Path, process::Command};

pub fn materialise(
    runtime: &Runtime,
    manifest: &Value,
    platform: &Value,
    directory: &Path,
) -> Result<BTreeMap<String, String>> {
    fs::create_dir_all(directory)?;
    let mut declared = serde_json::Map::new();
    for source in [manifest, platform] {
        if let Some(inputs) = source.get("inputs") {
            declared.extend(
                inputs
                    .as_object()
                    .context("release inputs must be an object")?
                    .clone(),
            );
        }
    }
    let mut mounts = super::mounts::validate(&declared, directory)?;
    let mut environment = BTreeMap::new();
    let mut receipts = Vec::new();
    for (key, entry) in declared {
        let uri = entry["uri"].as_str().context("release input has no URI")?;
        let coordinate = uri
            .strip_prefix("stado://sources/")
            .with_context(|| format!("input {key}: unsupported canonical source URI {uri}"))?;
        let parts: Vec<_> = coordinate.split('/').collect();
        let digest = entry["sha256"]
            .as_str()
            .context("release input has no SHA-256")?;
        if parts.len() < 3
            || parts[parts.len() - 2] != digest
            || digest.len() != 64
            || !digest
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            bail!(
                "input {key}: source URI must end in its declared SHA-256 and archive name: {uri}"
            );
        }
        let scope = parts[parts.len() - 3];
        if scope.is_empty()
            || !scope
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
        {
            bail!("input {key}: invalid canonical source name in {uri}");
        }
        let repository = if Some(scope) == manifest["product"].as_str() {
            key.replace('_', "-")
        } else {
            scope.to_owned()
        };
        let mount = mounts
            .remove(&key)
            .context("validated input mount is missing")?;
        fs::create_dir_all(mount.parent().context("input mount has no parent")?)?;
        let resolved = match source::checkout(runtime, &format!("wisent-ai/{repository}")) {
            Ok(checkout) => {
                #[cfg(unix)]
                std::os::unix::fs::symlink(&checkout, &mount)?;
                #[cfg(not(unix))]
                bail!("canonical source mounts require Unix symbolic links");
                let revision = source::revision(&checkout)?;
                eprintln!("input {key}: mounted {repository} at {revision} from the canonical checkout, not pinned archive {digest}");
                receipts.push(json!({"input": key, "kind": "canonical-checkout", "path": checkout, "revision": revision, "declared_archive": uri, "declared_sha256": digest}));
                checkout
            }
            Err(error) if error.is::<source::MissingCheckout>() => {
                let archive_name = parts.last().unwrap();
                let archive_name = relative(Path::new(archive_name))?;
                let downloads = directory.join(".downloads").join(&key);
                fs::create_dir_all(&downloads)?;
                let fetched = downloads.join(archive_name);
                checked(Command::new("stado").args(["storage", "get", uri]).arg(&fetched))
                    .with_context(|| format!("input {key}: no canonical checkout and the object store did not serve {uri}"))?;
                let actual = sha256(&fetched)?;
                if actual != digest {
                    bail!("input {key}: {uri} served SHA-256 {actual}; the manifest declares {digest}");
                }
                let resolved = if entry["extract"].as_bool().unwrap_or(false) {
                    fs::create_dir_all(&mount)?;
                    unpack(&fetched, &mount)?;
                    mount.clone()
                } else {
                    fs::copy(&fetched, &mount)?;
                    mount.parent().unwrap().to_path_buf()
                };
                receipts.push(json!({"input": key, "kind": "verified-archive", "uri": uri, "sha256": actual, "mount": mount}));
                resolved
            }
            Err(error) => return Err(error),
        };
        environment.insert(
            super::mounts::environment_name(&key),
            resolved.to_string_lossy().into_owned(),
        );
    }
    atomic_json(&directory.join("inputs.json"), &json!(receipts))?;
    Ok(environment)
}
