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
    let output = crate::wait::child_output(child, format!("skarbiec set-json {item}"))
        .map_err(|error| SkarbiecError::Deployment(error.to_string()))?;
    if !output.status.success() {
        return Err(SkarbiecError::Deployment(format!(
            "skarbiec could not store {item}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(())
}

/// The live items of the resolved owner vault, with their tags.
///
/// Reads and writes must use the same vault. Consulting the broker list here
/// can report an item absent while the owner vault already holds its signing
/// key, which would rotate that key during an otherwise idempotent bootstrap.
/// The installed `skarbiec` and the resolved owner vault are the whole read:
/// no host-specific wrapper is needed or consulted.
pub fn list_items() -> Result<Vec<crate::skarbiec::ItemInfo>, SkarbiecError> {
    owner_items()
}

fn owner_items() -> Result<Vec<crate::skarbiec::ItemInfo>, SkarbiecError> {
    let output = crate::wait::output(
        &mut std::process::Command::new(binary()?)
            .arg("list")
            .env("SKARBIEC_VAULT_FILE", vault()?)
            .env_remove("SKARBIEC_UNLOCK")
            .env_remove("SKARBIEC_UNLOCK_FILE"),
    )
    .map_err(|error| SkarbiecError::Deployment(error.to_string()))?;
    if !output.status.success() {
        return Err(SkarbiecError::Deployment(format!(
            "skarbiec could not list owner vault: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    let items: Vec<crate::skarbiec::ItemInfo> =
        serde_json::from_slice(&output.stdout).map_err(|error| {
            SkarbiecError::Deployment(format!("skarbiec owner list is not valid JSON: {error}"))
        })?;
    Ok(items
        .into_iter()
        .filter(|item| item.deleted != Some(true))
        .collect())
}

/// Check the owner vault itself for one live item.
pub fn item_exists(id: &str) -> Result<bool, SkarbiecError> {
    Ok(owner_items()?.iter().any(|item| item.id == id))
}

/// The id of the one live owner-vault item that plays `role`, or none.
/// Several items in one role are refused, because choosing would be a guess.
pub fn item_playing_role(role: &str) -> Result<Option<String>, SkarbiecError> {
    let items = owner_items()?;
    match crate::skarbiec::roles::holders(&items, role).as_slice() {
        [] => Ok(None),
        [one] => Ok(Some(one.id.clone())),
        several => Err(SkarbiecError::Deployment(format!(
            "{} owner-vault items carry {}; exactly one item may play role {role}",
            several.len(),
            crate::skarbiec::roles::role_tag(role)
        ))),
    }
}

/// One exact string field of the owner-vault item that plays `role`. A role
/// no item plays is refused naming the tag to put on the item, so a reader
/// never needs an item id.
pub fn read_role_string(role: &str, field: &str) -> Result<String, SkarbiecError> {
    let id = item_playing_role(role)?.ok_or_else(|| {
        SkarbiecError::Deployment(format!(
            "no owner-vault item carries {}; tag the item that plays role {role}",
            crate::skarbiec::roles::role_tag(role)
        ))
    })?;
    read_string(&id, field)
}

/// Write the item that plays `role` in the resolved owner vault: the one
/// holder is rewritten, and when none exists a new item is created under a
/// random id and tagged `stado:role:<role>`. Returns the item's id.
pub fn write_role_item(
    role: &str,
    item_type: &str,
    fields: &Value,
    context: &Value,
) -> Result<String, SkarbiecError> {
    if let Some(id) = item_playing_role(role)? {
        write_item(&id, item_type, fields, context)?;
        return Ok(id);
    }
    let id = crate::skarbiec::roles::fresh_item_id();
    write_item(&id, item_type, fields, context)?;
    let output = crate::wait::output(
        &mut std::process::Command::new(binary()?)
            .arg("retag")
            .arg(&id)
            .arg("--tags")
            .arg(crate::skarbiec::roles::role_tag(role))
            .env("SKARBIEC_VAULT_FILE", vault()?)
            .env_remove("SKARBIEC_UNLOCK")
            .env_remove("SKARBIEC_UNLOCK_FILE"),
    )
    .map_err(|error| SkarbiecError::Deployment(error.to_string()))?;
    if !output.status.success() {
        return Err(SkarbiecError::Deployment(format!(
            "skarbiec stored the {role} secret as {id} but could not tag it with its role: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(id)
}

/// Read one exact string field from the resolved owner vault.
///
/// The value is captured from stdin/stdout only and never enters argv. This is
/// the owner-side counterpart to broker reads for bootstrap credentials whose
/// workload grants deliberately exclude the Stado control process.
pub fn read_string(id: &str, field: &str) -> Result<String, SkarbiecError> {
    let output = crate::wait::output(
        &mut std::process::Command::new(binary()?)
            .arg("get")
            .arg(id)
            .env("SKARBIEC_VAULT_FILE", vault()?)
            .env_remove("SKARBIEC_UNLOCK")
            .env_remove("SKARBIEC_UNLOCK_FILE"),
    )
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

/// The whole item document (`fields`, `context`, …) from the resolved owner
/// vault, or none when the vault holds no such item. A rewrite that must keep
/// fields another owner put on the item reads them here.
pub fn read_document(id: &str) -> Result<Option<Value>, SkarbiecError> {
    if !item_exists(id)? {
        return Ok(None);
    }
    let output = crate::wait::output(
        &mut std::process::Command::new(binary()?)
            .arg("get")
            .arg(id)
            .env("SKARBIEC_VAULT_FILE", vault()?)
            .env_remove("SKARBIEC_UNLOCK")
            .env_remove("SKARBIEC_UNLOCK_FILE"),
    )
    .map_err(|error| SkarbiecError::Deployment(error.to_string()))?;
    if !output.status.success() {
        return Err(SkarbiecError::Deployment(format!(
            "skarbiec could not read {id}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    serde_json::from_slice(&output.stdout)
        .map(Some)
        .map_err(|error| SkarbiecError::Deployment(format!("{id} is not valid JSON: {error}")))
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
    let output = crate::wait::output(
        &mut std::process::Command::new(&binary)
            .arg("delete")
            .arg(id)
            .env("SKARBIEC_VAULT_FILE", &vault)
            .env_remove("SKARBIEC_UNLOCK")
            .env_remove("SKARBIEC_UNLOCK_FILE"),
    )
    .map_err(|error| SkarbiecError::Deployment(error.to_string()))?;
    if !output.status.success() {
        return Err(SkarbiecError::Deployment(format!(
            "skarbiec could not delete {id}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(())
}
