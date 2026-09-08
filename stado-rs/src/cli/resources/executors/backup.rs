//! Storage backup configuration: the one executor family that edits the
//! active Stado config file instead of calling a cloud API.

use std::fs;
use std::io::Write;
use std::path::Path;

use serde_json::{json, Value};
use tempfile::NamedTempFile;

use crate::cli::resources::model::Action;
use crate::cli::CmdError;

use super::conditions::conditions_match;

pub(super) fn inspect_backup_config() -> Result<Value, CmdError> {
    if backup_env_override() {
        return Ok(json!({
            "configured": true,
            "mutable": false,
            "reason": "backup configuration is overridden by environment variables",
        }));
    }
    let path = crate::config_file::config_path()?
        .ok_or_else(|| CmdError::click("no writable Stado config file is active"))?;
    let root: Value = serde_json::from_slice(&fs::read(&path)?)?;
    let backup = root.pointer("/storage/backup").cloned();
    Ok(json!({
        "configured": backup.as_ref().is_some_and(|value| !value.is_null()),
        "mutable": true,
        "path": path,
        "backup": backup,
    }))
}

pub(super) fn disable_backup_config(action: &Action) -> Result<Value, CmdError> {
    let state = inspect_backup_config()?;
    if state.get("mutable").and_then(Value::as_bool) != Some(true) {
        return Err(CmdError::click(
            state
                .get("reason")
                .and_then(Value::as_str)
                .unwrap_or("backup configuration is not mutable"),
        ));
    }
    if !conditions_match(&action.preconditions, &state) {
        return Err(CmdError::click(format!(
            "backup configuration drifted before action {}",
            action.id
        )));
    }
    let path = state
        .get("path")
        .and_then(Value::as_str)
        .ok_or_else(|| CmdError::click("backup config inspection returned no path"))?;
    let mut root: Value = serde_json::from_slice(&fs::read(path)?)?;
    if root.pointer("/storage/backup") != state.get("backup") {
        return Err(CmdError::click(format!(
            "backup configuration drifted while applying action {}",
            action.id
        )));
    }
    let storage = root
        .get_mut("storage")
        .and_then(Value::as_object_mut)
        .ok_or_else(|| CmdError::click("Stado config has no storage object"))?;
    let previous = storage.remove("backup").unwrap_or(Value::Null);
    atomic_json(Path::new(path), &root)?;
    Ok(json!({"previous_backup": previous, "config_path": path}))
}

pub(super) fn enable_backup_config(
    action: &Action,
    receipt: Option<&Value>,
) -> Result<Value, CmdError> {
    let path = receipt
        .and_then(|value| value.get("config_path"))
        .and_then(Value::as_str)
        .map(|value| Path::new(value).to_path_buf())
        .or(crate::config_file::config_path()?)
        .ok_or_else(|| CmdError::click("backup restore has no writable config path"))?;
    let backup = receipt
        .and_then(|value| value.get("previous_backup"))
        .or_else(|| action.parameters.get("previous"))
        .cloned()
        .filter(|value| !value.is_null())
        .ok_or_else(|| CmdError::click("backup restore has no previous value"))?;
    let mut root: Value = serde_json::from_slice(&fs::read(&path)?)?;
    if root
        .pointer("/storage/backup")
        .is_some_and(|value| !value.is_null())
    {
        return Err(CmdError::click(
            "backup configuration was replaced before restore",
        ));
    }
    let storage = root
        .get_mut("storage")
        .and_then(Value::as_object_mut)
        .ok_or_else(|| CmdError::click("Stado config has no storage object"))?;
    storage.insert("backup".to_string(), backup);
    atomic_json(&path, &root)?;
    Ok(json!({"restored": true, "config_path": path}))
}

fn backup_env_override() -> bool {
    crate::capabilities::STORAGE_BACKEND_CONFIG
        .backup_env
        .into_iter()
        .chain(crate::capabilities::backup_config_envs(
            crate::capabilities::RuntimeFacet::Storage,
        ))
        .any(|name| std::env::var(name).is_ok_and(|value| !value.trim().is_empty()))
}

fn atomic_json(path: &Path, value: &Value) -> Result<(), CmdError> {
    let parent = path
        .parent()
        .ok_or_else(|| CmdError::click(format!("{} has no parent", path.display())))?;
    let mut bytes = serde_json::to_vec_pretty(value)?;
    bytes.push(b'\n');
    let mut temporary = NamedTempFile::new_in(parent)?;
    temporary.write_all(&bytes)?;
    temporary.as_file().sync_all()?;
    temporary.persist(path).map_err(|error| error.error)?;
    fs::File::open(parent)?.sync_all()?;
    Ok(())
}
