//! Persisting the credential-store selector a migration commits: the config
//! file it lands in, written whole and flushed with its directory.

use super::deployment;
use crate::skarbiec::SkarbiecError;
use serde_json::{Map, Value};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

fn config_path_for_write() -> Result<PathBuf, SkarbiecError> {
    // The selected file is written even before it exists; a selection that
    // is not text is refused rather than read as none.
    match std::env::var(crate::config_file::FILE_ENV) {
        Ok(raw) if !raw.trim().is_empty() => {
            return Ok(crate::config_file::expand_tilde(raw.trim()))
        }
        Ok(_) | Err(std::env::VarError::NotPresent) => {}
        Err(error) => {
            return Err(deployment(format!(
                "{} cannot be read: {error}",
                crate::config_file::FILE_ENV
            )))
        }
    }
    if let Some(path) =
        crate::config_file::find_config_file().map_err(|error| deployment(error.to_string()))?
    {
        return Ok(path);
    }
    let home = crate::config_file::home()
        .map_err(|cause| deployment(format!("cannot persist credentials.store: {cause}")))?;
    Ok(home.join(".stado").join("config.json"))
}

#[cfg(unix)]
fn write_config(path: &Path, body: &[u8]) -> Result<(), SkarbiecError> {
    // A config that exists keeps its mode; a new one is the owner's alone. A
    // config whose metadata cannot be read is refused, not taken as absent.
    let mode = match std::fs::metadata(path) {
        Ok(metadata) => {
            metadata.permissions().mode() & crate::primitives::file_mode::permission_bits()
        }
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
            crate::primitives::file_mode::owner_read_write()
        }
        Err(source) => {
            return Err(deployment(format!(
                "cannot read the mode of config {}: {source}",
                path.display()
            )))
        }
    };
    crate::primitives::document::persist_with_mode(path, body, mode).map_err(|source| {
        deployment(format!("cannot replace config {}: {source}", path.display()))
    })
}

#[cfg(not(unix))]
fn write_config(path: &Path, body: &[u8]) -> Result<(), SkarbiecError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|source| deployment(format!("cannot create config directory: {source}")))?;
    }
    std::fs::write(path, body)
        .map_err(|source| deployment(format!("cannot write config {}: {source}", path.display())))
}

/// Write `locator` as `credentials.store` into the config file, keeping
/// every other field the file holds.
pub(super) fn persist_selector(locator: &str) -> Result<(), SkarbiecError> {
    let path = config_path_for_write()?;
    // Only a config that does not exist starts empty: one that cannot be
    // read is refused with its cause.
    let mut document = match std::fs::read_to_string(&path) {
        Ok(body) => serde_json::from_str::<Value>(&body).map_err(|source| {
            deployment(format!(
                "config {} is invalid JSON: {source}",
                path.display()
            ))
        })?,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Value::Object(Map::new()),
        Err(source) => {
            return Err(deployment(format!(
                "cannot read config {}: {source}",
                path.display()
            )))
        }
    };
    let root = document
        .as_object_mut()
        .ok_or_else(|| deployment(format!("config {} must be a JSON object", path.display())))?;
    match root.get_mut("credentials") {
        None => {
            root.insert(
                "credentials".to_string(),
                serde_json::json!({"store": locator}),
            );
        }
        Some(Value::Object(credentials)) => {
            credentials.insert("store".to_string(), Value::String(locator.to_string()));
        }
        Some(_) => {
            return Err(deployment(format!(
                "config {} field credentials must be an object",
                path.display()
            )));
        }
    }
    let body = format!(
        "{}\n",
        serde_json::to_string_pretty(&document).map_err(|source| deployment(source.to_string()))?
    );
    write_config(&path, body.as_bytes())
}
