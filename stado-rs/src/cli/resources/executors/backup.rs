//! Storage backup configuration: the one executor family that edits the
//! active Stado config file instead of calling a cloud API.

use std::fs;
use std::path::Path;

use serde_json::{json, Value};

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
    let path = crate::config_file::config_path()?.ok_or_else(|| {
        CmdError::click("no writable Stado config file is active")
            .stating(crate::primitives::failure::FailureCode::Config)
    })?;
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
        // The inspection names why the configuration cannot be changed; an
        // inspection that withheld the reason is refused by that.
        return Err(CmdError::refused(
            match state.get("reason").and_then(Value::as_str) {
                Some(reason) => reason.to_string(),
                None => "backup configuration is not mutable, and the inspection named no reason"
                    .to_string(),
            },
        ));
    }
    if !conditions_match(&action.preconditions, &state) {
        return Err(CmdError::refused(format!(
            "backup configuration drifted before action {}",
            action.id
        )));
    }
    let path = state
        .get("path")
        .and_then(Value::as_str)
        .ok_or_else(|| CmdError::unreachable("backup config inspection returned no path"))?;
    let mut root: Value = serde_json::from_slice(&fs::read(path)?)?;
    if root.pointer("/storage/backup") != state.get("backup") {
        return Err(CmdError::refused(format!(
            "backup configuration drifted while applying action {}",
            action.id
        )));
    }
    let storage = root
        .get_mut("storage")
        .and_then(Value::as_object_mut)
        .ok_or_else(|| {
            CmdError::click("Stado config has no storage object")
                .stating(crate::primitives::failure::FailureCode::Config)
        })?;
    // A `storage` object without a `backup` block had nothing to disable;
    // the receipt records that as `null` rather than inventing a block.
    let previous = storage.remove("backup");
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
        .ok_or_else(|| {
            CmdError::click("backup restore has no writable config path")
                .stating(crate::primitives::failure::FailureCode::Config)
        })?;
    let backup = receipt
        .and_then(|value| value.get("previous_backup"))
        .or_else(|| action.parameters.get("previous"))
        .cloned()
        .filter(|value| !value.is_null())
        .ok_or_else(|| CmdError::refused("backup restore has no previous value"))?;
    let mut root: Value = serde_json::from_slice(&fs::read(&path)?)?;
    if root
        .pointer("/storage/backup")
        .is_some_and(|value| !value.is_null())
    {
        return Err(CmdError::refused(
            "backup configuration was replaced before restore",
        ));
    }
    let storage = root
        .get_mut("storage")
        .and_then(Value::as_object_mut)
        .ok_or_else(|| {
            CmdError::click("Stado config has no storage object")
                .stating(crate::primitives::failure::FailureCode::Config)
        })?;
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

/// The active config file replaced whole through Stado's one durable writer
/// ([`crate::primitives::document::persist`]), with the trailing newline a
/// hand-edited config keeps.
fn atomic_json(path: &Path, value: &Value) -> Result<(), CmdError> {
    let mut bytes = serde_json::to_vec_pretty(value)?;
    bytes.push(b'\n');
    crate::primitives::document::persist(path, &bytes)?;
    Ok(())
}
