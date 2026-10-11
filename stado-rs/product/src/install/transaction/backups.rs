//! The copies an install keeps of what it replaces, made durable before the
//! replacement is placed, and their fingerprints.

use super::super::plan::Placement;
use super::destination;
use crate::{
    common::{copy_tree, Runtime},
    state::Backup,
};
use anyhow::{Context, Result};
use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
};

pub fn backup(runtime: &Runtime, product: &str, paths: &[PathBuf]) -> Result<Vec<Backup>> {
    let root = runtime
        .home
        .join(".stado/products")
        .join(product)
        .join("backups")
        .join(uuid::Uuid::new_v4().to_string());
    fs::create_dir_all(&root)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700))?;
    }
    let mut backups = Vec::new();
    for (index, path) in paths.iter().enumerate() {
        destination(runtime, path)?;
        match path.symlink_metadata() {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error.into()),
            Ok(_) => (),
        }
        let directory = root.join(index.to_string());
        fs::create_dir(&directory)?;
        let backup = directory.join(path.file_name().context("backup source has no filename")?);
        copy_tree(path, &backup)?;
        sync_tree(&backup)?;
        fs::File::open(&directory)?.sync_all()?;
        backups.push(Backup {
            path: path.clone(),
            backup,
        });
    }
    fs::File::open(&root)?.sync_all()?;
    Ok(backups)
}

pub fn backup_fingerprints(backups: &[Backup]) -> Result<Value> {
    let mut fingerprints = serde_json::Map::new();
    for saved in backups {
        fingerprints.insert(
            saved.backup.to_string_lossy().into_owned(),
            Placement {
                source: saved.backup.clone(),
                destination: saved.path.clone(),
                symbolic: false,
            }
            .fingerprint()?,
        );
    }
    Ok(Value::Object(fingerprints))
}

/// Flush `path` and, for a directory, everything under it to disk; a
/// symbolic link is its own entry and is not followed.
pub(super) fn sync_tree(path: &Path) -> Result<()> {
    let metadata = path.symlink_metadata()?;
    if metadata.file_type().is_symlink() {
        return Ok(());
    }
    if metadata.is_dir() {
        for entry in fs::read_dir(path)? {
            sync_tree(&entry?.path())?;
        }
    }
    fs::File::open(path)?.sync_all()?;
    Ok(())
}
