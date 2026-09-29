use crate::{
    common::{atomic_json, checked, relative, sha256, stado, unpack, Runtime},
    source,
};
use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::{collections::BTreeMap, fs, path::Path};

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
        let digest = entry["sha256"]
            .as_str()
            .context("release input has no SHA-256")?;
        let mount = mounts
            .remove(&key)
            .context("validated input mount is missing")?;
        fs::create_dir_all(mount.parent().context("input mount has no parent")?)?;
        // A published release of another product (`stado://releases/<product>/
        // <version>/<platform>/<archive>`) is immutable and has no checkout to
        // stand in for it: it is always fetched and held to its declared digest,
        // exactly as the release worker does.
        if uri.starts_with("stado://releases/") {
            let (resolved, receipt) = fetch_verified(&key, uri, digest, &entry, directory, &mount)?;
            receipts.push(receipt);
            environment.insert(
                super::mounts::environment_name(&key),
                resolved.to_string_lossy().into_owned(),
            );
            continue;
        }
        let coordinate = uri.strip_prefix("stado://sources/").with_context(|| {
            format!("input {key}: {uri} is neither stado://sources/ nor stado://releases/")
        })?;
        let parts: Vec<_> = coordinate.split('/').collect();
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
        // A checkout stands in for an extracted source tree only. An input the
        // build reads as one file (`extract: false`, such as a git bundle) has
        // no checkout shape: a mounted directory there makes the build's
        // `install` refuse it, so that archive is fetched and held to its digest.
        let substitute = extracted(&entry);
        let checkout = if substitute {
            match source::checkout(runtime, &format!("wisent-ai/{repository}")) {
                Ok(checkout) => Some(checkout),
                Err(error) if error.is::<source::MissingCheckout>() => None,
                Err(error) => return Err(error),
            }
        } else {
            None
        };
        let resolved = match checkout {
            Some(checkout) => {
                #[cfg(unix)]
                std::os::unix::fs::symlink(&checkout, &mount)?;
                #[cfg(not(unix))]
                bail!("canonical source mounts require Unix symbolic links");
                let revision = source::revision(&checkout)?;
                eprintln!("input {key}: mounted {repository} at {revision} from the canonical checkout, not pinned archive {digest}");
                receipts.push(json!({"input": key, "kind": "canonical-checkout", "path": checkout, "revision": revision, "declared_archive": uri, "declared_sha256": digest}));
                checkout
            }
            None => {
                let (resolved, receipt) =
                    fetch_verified(&key, uri, digest, &entry, directory, &mount)?;
                receipts.push(receipt);
                resolved
            }
        };
        environment.insert(
            super::mounts::environment_name(&key),
            resolved.to_string_lossy().into_owned(),
        );
    }
    atomic_json(&directory.join("inputs.json"), &json!(receipts))?;
    Ok(environment)
}

/// Fetch one declared input archive from the object store, refuse it unless it
/// has the declared SHA-256, and mount it: unpacked when `extract` is set,
/// otherwise copied to the mount path. Returns what the build sees and the
/// receipt that records it.
fn fetch_verified(
    key: &str,
    uri: &str,
    digest: &str,
    entry: &Value,
    directory: &Path,
    mount: &Path,
) -> Result<(std::path::PathBuf, Value)> {
    let archive_name = uri
        .rsplit('/')
        .next()
        .filter(|name| !name.is_empty())
        .with_context(|| format!("input {key}: {uri} names no archive"))?;
    let archive_name = relative(Path::new(archive_name))?;
    let downloads = directory.join(".downloads").join(key);
    fs::create_dir_all(&downloads)?;
    let fetched = downloads.join(archive_name);
    checked(stado().args(["storage", "get", uri]).arg(&fetched))
        .with_context(|| format!("input {key}: the object store did not serve {uri}"))?;
    let actual = sha256(&fetched)?;
    if actual != digest {
        bail!("input {key}: {uri} served SHA-256 {actual}; the manifest declares {digest}");
    }
    // The build reads `WISENT_INPUT_<KEY>_DIR` as the mount itself, the way the
    // release worker sets it (`inputs_root/<mount>`): an unpacked tree, or the
    // one file an unextracted input is. Naming the file's parent directory
    // instead gave every single-file input of a product the same value.
    if extracted(entry) {
        fs::create_dir_all(mount)?;
        unpack(&fetched, mount)?;
    } else {
        fs::copy(&fetched, mount)?;
    }
    let resolved = mount.to_path_buf();
    let receipt = json!({"input": key, "kind": "verified-archive", "uri": uri, "sha256": actual, "mount": mount});
    Ok((resolved, receipt))
}

/// Whether an input is unpacked. Absent means yes, as the release contract
/// (`release_pipeline::validate::predicates::default_extract`) and the release
/// worker read it: a source archive declared without the field is a tree.
fn extracted(entry: &Value) -> bool {
    entry["extract"].as_bool().unwrap_or(true)
}
