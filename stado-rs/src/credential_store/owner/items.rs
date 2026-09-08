//! The item calls themselves: one write against an explicit vault, and the
//! reads and writes that go through the resolved owner vault.

use std::io::Write;
use std::path::Path;

use serde_json::{json, Value};

use crate::skarbiec::SkarbiecError;

use super::discovery::binary;
use super::resolution::vault;

/// Envelope every owner write carries.
const ITEM_SCHEMA: &str = "skarbiec.item.v2";

/// Write one item into an explicit vault through its owner.
///
/// `set-json` takes a canonical payload and validates it: the kind must be one
/// Skarbiec declares, every key in `fields` must be one that kind allows, and
/// anything descriptive belongs in `context`. So `ssh-key` is not a kind — it is
/// a `key-pair` whose fingerprint and key type are context — and passing the
/// wrong one is refused rather than stored in a shape no reader expects.
///
/// `SKARBIEC_UNLOCK`/`SKARBIEC_UNLOCK_FILE` are removed for the child: an unlock
/// phrase inherited from this process's environment would decide which vault key
/// is used without any caller having asked for it. The payload travels on stdin,
/// never in argv, because argv is readable by every process on the machine.
pub fn store_json(
    binary: &Path,
    vault: &Path,
    item: &str,
    item_type: &str,
    fields: &Value,
    context: &Value,
) -> Result<(), SkarbiecError> {
    let mut child = std::process::Command::new(binary)
        .arg("set-json")
        .arg(item)
        .arg("--type")
        .arg(item_type)
        .env("SKARBIEC_VAULT_FILE", vault)
        .env_remove("SKARBIEC_UNLOCK")
        .env_remove("SKARBIEC_UNLOCK_FILE")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|error| SkarbiecError::Deployment(error.to_string()))?;
    let payload = json!({
        "schema": ITEM_SCHEMA,
        "kind": item_type,
        "fields": fields,
        "context": context,
    });
    if let Some(stdin) = child.stdin.as_mut() {
        stdin
            .write_all(payload.to_string().as_bytes())
            .map_err(|error| SkarbiecError::Deployment(error.to_string()))?;
    }
    let output = child
        .wait_with_output()
        .map_err(|error| SkarbiecError::Deployment(error.to_string()))?;
    if !output.status.success() {
        return Err(SkarbiecError::Deployment(format!(
            "skarbiec could not store {item}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(())
}

/// Check the owner vault itself for one live item.
///
/// Reads and writes must use the same vault. Consulting the broker list here
/// can report an item absent while the owner vault already holds its signing
/// key, which would rotate that key during an otherwise idempotent bootstrap.
pub fn item_exists(id: &str) -> Result<bool, SkarbiecError> {
    let output = std::process::Command::new(binary()?)
        .arg("list")
        .env("SKARBIEC_VAULT_FILE", vault()?)
        .env_remove("SKARBIEC_UNLOCK")
        .env_remove("SKARBIEC_UNLOCK_FILE")
        .output()
        .map_err(|error| SkarbiecError::Deployment(error.to_string()))?;
    if !output.status.success() {
        return Err(SkarbiecError::Deployment(format!(
            "skarbiec could not list owner vault: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    let items: Vec<Value> = serde_json::from_slice(&output.stdout).map_err(|error| {
        SkarbiecError::Deployment(format!("skarbiec owner list is not valid JSON: {error}"))
    })?;
    Ok(items.iter().any(|item| {
        item.get("id").and_then(Value::as_str) == Some(id)
            && item.get("deleted").and_then(Value::as_bool) != Some(true)
    }))
}

/// Read one exact string field from the resolved owner vault.
///
/// The value is captured from stdin/stdout only and never enters argv. This is
/// the owner-side counterpart to broker reads for bootstrap credentials whose
/// workload grants deliberately exclude the Stado control process.
pub fn read_string(id: &str, field: &str) -> Result<String, SkarbiecError> {
    let output = std::process::Command::new(binary()?)
        .arg("get")
        .arg(id)
        .env("SKARBIEC_VAULT_FILE", vault()?)
        .env_remove("SKARBIEC_UNLOCK")
        .env_remove("SKARBIEC_UNLOCK_FILE")
        .output()
        .map_err(|error| SkarbiecError::Deployment(error.to_string()))?;
    if !output.status.success() {
        return Err(SkarbiecError::Deployment(format!(
            "skarbiec could not read {id}.{field}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    let document: Value = serde_json::from_slice(&output.stdout)
        .map_err(|error| SkarbiecError::Deployment(format!("{id} is not valid JSON: {error}")))?;
    let value = document
        .get("fields")
        .and_then(Value::as_object)
        .and_then(|fields| fields.get(field))
        .and_then(Value::as_str)
        .ok_or_else(|| SkarbiecError::Deployment(format!("{id} has no string field {field}")))?
        .to_string();
    if value.is_empty() {
        return Err(SkarbiecError::Deployment(format!("{id}.{field} is empty")));
    }
    Ok(value)
}

/// Write one item into the resolved owner vault.
pub fn write_item(
    id: &str,
    item_type: &str,
    fields: &Value,
    context: &Value,
) -> Result<(), SkarbiecError> {
    store_json(&binary()?, &vault()?, id, item_type, fields, context)
}

/// Delete one item from the resolved owner vault.
pub fn delete_item(id: &str) -> Result<(), SkarbiecError> {
    let binary = binary()?;
    let vault = vault()?;
    let output = std::process::Command::new(&binary)
        .arg("delete")
        .arg(id)
        .env("SKARBIEC_VAULT_FILE", &vault)
        .env_remove("SKARBIEC_UNLOCK")
        .env_remove("SKARBIEC_UNLOCK_FILE")
        .output()
        .map_err(|error| SkarbiecError::Deployment(error.to_string()))?;
    if !output.status.success() {
        return Err(SkarbiecError::Deployment(format!(
            "skarbiec could not delete {id}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(())
}
