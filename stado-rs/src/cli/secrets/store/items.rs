//! Field delivery and its refusals: one item in, one item or one exact string
//! field out, the visible inventory, and one removal.

use std::io::Read;

use serde_json::{json, Value};

use crate::cli::{table, CmdError};

use crate::cli::secrets::store::resolve::unknown;

fn read_value_from_stdin() -> Result<String, CmdError> {
    let mut value = String::new();
    std::io::stdin().read_to_string(&mut value)?;
    let value = value.strip_suffix('\n').unwrap_or(&value);
    Ok(value.strip_suffix('\r').unwrap_or(value).to_string())
}

pub(crate) async fn put(
    vault: &crate::skarbiec::Client,
    name: &str,
    item_type: Option<&str>,
) -> Result<(), CmdError> {
    let input = read_value_from_stdin()?;
    if input.is_empty() {
        return Err(CmdError::click(
            "stdin was empty; pipe the value in (stado secrets put NAME < file)",
        ));
    }
    let value: Value = serde_json::from_str(&input).unwrap_or_else(|_| json!({"value": input}));
    // The kind is the payload's shape, so the payload decides it when it says
    // so. Forcing `stado-secret` on every write is how one item ends up holding
    // a key pair with no schema requiring its public half.
    let declared = value
        .get("kind")
        .and_then(Value::as_str)
        .filter(|kind| !kind.trim().is_empty());
    let item_kind = item_type.or(declared).unwrap_or("stado-secret");
    vault
        .write_item(name, item_kind, &value)
        .await
        .map_err(|err| CmdError::click(err.to_string()))?;
    println!("stored credential item {name:?} as {item_kind:?}");
    Ok(())
}

pub(crate) async fn get(
    vault: &crate::skarbiec::Client,
    name: &str,
    field: Option<&str>,
) -> Result<(), CmdError> {
    if let Some(field) = field {
        let raw = vault
            .read_string(name, field)
            .await
            .map_err(|err| CmdError::click(err.to_string()))?
            .filter(|raw| !raw.is_empty())
            .ok_or_else(|| {
                CmdError::click(format!(
                    "credential item {name:?} has no non-empty string field {field:?}"
                ))
            })?;
        println!("{raw}");
        return Ok(());
    }
    let value = vault.read_item(name).await.map_err(|err| {
        CmdError::click(format!(
            "{err}; this store answers per field: name one with --field"
        ))
    })?;
    if let Some(object) = value.as_object() {
        if object.len() == usize::from(true) {
            if let Some(raw) = object.get("value").and_then(Value::as_str) {
                println!("{raw}");
                return Ok(());
            }
        }
    }
    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(())
}

pub(crate) async fn ls(vault: &crate::skarbiec::Client, as_json: bool) -> Result<(), CmdError> {
    let stored = vault
        .list_items()
        .await
        .map_err(|err| CmdError::click(err.to_string()))?;
    if as_json {
        println!("{}", serde_json::to_string_pretty(&stored)?);
        return Ok(());
    }
    if stored.is_empty() {
        println!("No credential items are visible to this store administrator.");
        return Ok(());
    }
    let rows: Vec<Vec<String>> = stored
        .iter()
        .map(|item| {
            vec![
                item.id.clone(),
                item.item_type.clone().unwrap_or_else(unknown),
                item.updated_at
                    .map(|at| at.to_rfc3339())
                    .unwrap_or_else(unknown),
                item.versions
                    .map(|versions| versions.to_string())
                    .unwrap_or_else(unknown),
            ]
        })
        .collect();
    table::print(&["NAME", "TYPE", "UPDATED", "VERSIONS"], &rows);
    Ok(())
}

pub(crate) async fn rm(vault: &crate::skarbiec::Client, name: &str) -> Result<(), CmdError> {
    vault
        .delete_item(name)
        .await
        .map_err(|err| CmdError::click(err.to_string()))?;
    println!("removed credential item {name:?}");
    Ok(())
}
