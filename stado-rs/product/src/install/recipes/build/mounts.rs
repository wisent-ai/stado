use crate::common::{absolute, relative};
use anyhow::{bail, Context, Result};
use serde_json::{Map, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

pub fn environment_name(key: &str) -> String {
    format!(
        "WISENT_INPUT_{}_DIR",
        key.to_ascii_uppercase().replace('-', "_")
    )
}

pub fn validate(
    declared: &Map<String, Value>,
    directory: &Path,
) -> Result<BTreeMap<String, PathBuf>> {
    if absolute(directory)? != directory.canonicalize()? {
        bail!(
            "build input root is not a physical canonical directory: {}",
            directory.display()
        );
    }
    let mut mounts = BTreeMap::<String, PathBuf>::new();
    let mut variables = BTreeSet::new();
    let downloads = directory.join(".downloads");
    let receipt = directory.join("inputs.json");
    for (key, entry) in declared {
        if key.is_empty()
            || !key
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
        {
            bail!("invalid input environment name {key}");
        }
        if !variables.insert(environment_name(key)) {
            bail!("build inputs resolve to the same environment variable: {key}");
        }
        let mount = directory.join(relative(Path::new(entry["mount"].as_str().unwrap_or(key)))?);
        if mount.starts_with(&downloads)
            || downloads.starts_with(&mount)
            || mount.starts_with(&receipt)
            || receipt.starts_with(&mount)
        {
            bail!(
                "build input {key} overlaps installer evidence: {}",
                mount.display()
            );
        }
        for (other, previous) in &mounts {
            if mount.starts_with(previous) || previous.starts_with(&mount) {
                bail!("build inputs {key} and {other} have overlapping mounts");
            }
        }
        match mount.symlink_metadata() {
            Ok(_) => bail!(
                "build input destination already exists: {}",
                mount.display()
            ),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("inspect input mount {}", mount.display()))
            }
        }
        let mut parent = mount.parent();
        while let Some(path) = parent.filter(|path| *path != directory) {
            match path.symlink_metadata() {
                Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => bail!(
                    "build input parent is not a physical directory: {}",
                    path.display()
                ),
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(error)
                        .with_context(|| format!("inspect input parent {}", path.display()))
                }
            }
            parent = path.parent();
        }
        mounts.insert(key.clone(), mount);
    }
    Ok(mounts)
}
