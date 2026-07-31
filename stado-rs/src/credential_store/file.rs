//! Owner and mode checks plus reads for the local file credential backend.

#[cfg(unix)]
use std::io::Write;
use std::path::Path;
#[cfg(unix)]
use std::sync::LazyLock;

use serde_json::{Map, Value};

use crate::skarbiec::SkarbiecError;

pub(super) const TYPE_METADATA: &str = "$stado_item_types";

/// Effective uid of this process, resolved once via `id` (no numeric literal
/// and no extra crate feature; matches the Skarbiec-side precedent).
#[cfg(unix)]
fn current_uid() -> Result<u32, SkarbiecError> {
    static UID: LazyLock<Result<u32, String>> = LazyLock::new(|| {
        let output = std::process::Command::new("id")
            .arg("-u")
            .output()
            .map_err(|error| error.to_string())?;
        if !output.status.success() {
            return Err("id -u exited non-zero".to_string());
        }
        String::from_utf8_lossy(&output.stdout)
            .trim()
            .parse()
            .map_err(|error| format!("cannot parse id -u output: {error}"))
    });
    UID.as_ref().copied().map_err(|detail| {
        SkarbiecError::Deployment(format!("cannot determine current uid: {detail}"))
    })
}

/// The store file must be a regular, non-symlink file owned by the current
/// user with owner-only mode bits — the checks `skarbiec-vault-publish`'s
/// `checkedOwnerFile` applies to the vault before publishing it.
#[cfg(unix)]
pub(super) fn checked_owner_file(path: &Path) -> Result<(), SkarbiecError> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    let insecure = |reason: &str| {
        SkarbiecError::Deployment(format!(
            "credential store file {}: {reason}",
            path.display()
        ))
    };
    let metadata = std::fs::symlink_metadata(path).map_err(|source| {
        SkarbiecError::Deployment(format!(
            "cannot read credential store file {}: {source}",
            path.display()
        ))
    })?;
    if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
        return Err(insecure(
            "must be a regular file, not a symlink or special file",
        ));
    }
    if metadata.uid() != current_uid()? {
        return Err(insecure("must be owned by the current user"));
    }
    let non_owner_mask = u32::from(u8::MAX >> (u16::BITS / u8::BITS));
    if metadata.permissions().mode() & non_owner_mask != u32::MIN {
        return Err(insecure("must not be accessible by group or other users"));
    }
    Ok(())
}

#[cfg(not(unix))]
pub(super) fn checked_owner_file(_path: &Path) -> Result<(), SkarbiecError> {
    Ok(())
}

fn read_store_file(path: &Path) -> Result<Value, SkarbiecError> {
    checked_owner_file(path)?;
    let body = std::fs::read_to_string(path).map_err(|source| {
        SkarbiecError::Deployment(format!(
            "cannot read credential store file {}: {source}",
            path.display()
        ))
    })?;
    let doc: Value = serde_json::from_str(&body).map_err(|source| {
        SkarbiecError::Deployment(format!(
            "credential store file {} is not a JSON object of items: {source}",
            path.display()
        ))
    })?;
    if !doc.is_object() {
        return Err(SkarbiecError::Deployment(format!(
            "credential store file {} top level must be an object of items",
            path.display()
        )));
    }
    Ok(doc)
}

/// A missing item mirrors the Skarbiec read path: `read_item` reports
/// `MissingValue`, while `read_string` resolves it to `None`.
pub(super) fn file_read_item(path: &Path, id: &str) -> Result<Value, SkarbiecError> {
    read_store_file(path)?
        .get(id)
        .cloned()
        .ok_or_else(|| SkarbiecError::MissingValue(id.to_string()))
}

pub(super) fn file_read_string(
    path: &Path,
    id: &str,
    field: &str,
) -> Result<Option<String>, SkarbiecError> {
    Ok(read_store_file(path)?
        .get(id)
        .and_then(|item| item.get(field))
        .and_then(Value::as_str)
        .map(str::to_string))
}
pub(super) fn file_load(path: &Path) -> Result<Value, SkarbiecError> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => {
            checked_owner_file(path)?;
            let body = std::fs::read_to_string(path).map_err(|source| {
                SkarbiecError::Deployment(format!(
                    "cannot read credential store file {}: {source}",
                    path.display()
                ))
            })?;
            let document: Value = serde_json::from_str(&body).map_err(|source| {
                SkarbiecError::Deployment(format!(
                    "credential store file {} is not valid JSON: {source}",
                    path.display()
                ))
            })?;
            if !document.is_object() {
                return Err(SkarbiecError::Deployment(format!(
                    "credential store file {} top level must be an object",
                    path.display()
                )));
            }
            if document
                .get(TYPE_METADATA)
                .is_some_and(|metadata| !metadata.is_object())
            {
                return Err(SkarbiecError::Deployment(format!(
                    "credential store file {} field {TYPE_METADATA:?} must be an object",
                    path.display()
                )));
            }
            Ok(document)
        }
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
            Ok(Value::Object(Map::new()))
        }
        Err(source) => Err(SkarbiecError::Deployment(format!(
            "cannot inspect credential store file {}: {source}",
            path.display()
        ))),
    }
}

#[cfg(unix)]
pub(super) fn file_store(path: &Path, doc: &Value) -> Result<(), SkarbiecError> {
    use std::os::unix::fs::OpenOptionsExt;

    let parent = path.parent().ok_or_else(|| {
        SkarbiecError::Deployment(format!(
            "credential store path {} has no parent",
            path.display()
        ))
    })?;
    std::fs::create_dir_all(parent).map_err(|source| {
        SkarbiecError::Deployment(format!(
            "cannot create credential store directory {}: {source}",
            parent.display()
        ))
    })?;
    let temporary = parent.join(format!(
        ".stado-credentials-{}-{}.tmp",
        std::process::id(),
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("store")
    ));
    let owner_mode = u32::from_str_radix("600", u32::from(u8::BITS))
        .map_err(|source| SkarbiecError::Deployment(source.to_string()))?;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(owner_mode)
        .open(&temporary)
        .map_err(|source| {
            SkarbiecError::Deployment(format!(
                "cannot create temporary credential store {}: {source}",
                temporary.display()
            ))
        })?;
    let body = serde_json::to_string_pretty(doc)
        .map_err(|source| SkarbiecError::Deployment(source.to_string()))?;
    file.write_all(format!("{body}\n").as_bytes())
        .and_then(|_| file.sync_all())
        .map_err(|source| {
            SkarbiecError::Deployment(format!(
                "cannot write temporary credential store {}: {source}",
                temporary.display()
            ))
        })?;
    std::fs::rename(&temporary, path).map_err(|source| {
        let _ = std::fs::remove_file(&temporary);
        SkarbiecError::Deployment(format!(
            "cannot replace credential store {}: {source}",
            path.display()
        ))
    })
}

#[cfg(not(unix))]
pub(super) fn file_store(path: &Path, doc: &Value) -> Result<(), SkarbiecError> {
    let body = serde_json::to_string_pretty(doc)
        .map_err(|source| SkarbiecError::Deployment(source.to_string()))?;
    std::fs::write(path, format!("{body}\n")).map_err(|source| {
        SkarbiecError::Deployment(format!(
            "cannot write credential store file {}: {source}",
            path.display()
        ))
    })
}
