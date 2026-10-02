//! Field delivery and its refusals: one item in, one item or one exact string
//! field out, the visible inventory, and one removal — against the store
//! `credentials.store` selects. A file store needs no Skarbiec: these verbs
//! read and write that file, and only a Skarbiec store builds the store
//! administrator's client.

use std::io::Read;
use std::path::PathBuf;

use serde_json::{json, Value};

use crate::cli::{reporting::table, CmdError};
use crate::credential_store::Backend;
use crate::skarbiec::Client;

use crate::cli::secrets::store::resolve::{client, unknown};

/// Where `stado credentials get|put|ls|rm` act.
pub(crate) enum Store {
    File(PathBuf),
    Skarbiec(Client),
}

fn refused(error: impl std::fmt::Display) -> CmdError {
    CmdError::click(error.to_string())
}

/// The selected store; a Skarbiec store is reached with the store
/// administrator's grant.
pub(crate) fn store() -> Result<Store, CmdError> {
    match crate::credential_store::selected().map_err(refused)? {
        Backend::File { path } => Ok(Store::File(path)),
        Backend::Skarbiec { .. } => Ok(Store::Skarbiec(client()?)),
    }
}

fn read_value_from_stdin() -> Result<String, CmdError> {
    let mut value = String::new();
    std::io::stdin().read_to_string(&mut value)?;
    let value = value.strip_suffix('\n').unwrap_or(&value);
    Ok(value.strip_suffix('\r').unwrap_or(value).to_string())
}

pub(crate) async fn put(
    store: &Store,
    name: &str,
    item_type: Option<&str>,
) -> Result<(), CmdError> {
    let input = read_value_from_stdin()?;
    if input.is_empty() {
        return Err(CmdError::click(
            "stdin was empty; pipe the value in (stado credentials put NAME < file)",
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
    match store {
        Store::Skarbiec(vault) => vault
            .write_item(name, item_kind, &value)
            .await
            .map_err(refused)?,
        Store::File(path) => crate::credential_store::write::write_item_at(
            &Backend::File { path: path.clone() },
            name,
            item_kind,
            &value,
            &Value::Null,
        )
        .await
        .map_err(refused)?,
    }
    println!("stored credential item {name:?} as {item_kind:?}");
    Ok(())
}

pub(crate) async fn get(store: &Store, name: &str, field: Option<&str>) -> Result<(), CmdError> {
    if let Some(field) = field {
        let raw = match store {
            Store::Skarbiec(vault) => vault.read_declared_string(name, field).await,
            Store::File(_) => crate::credential_store::read_string(name, field).await,
        }
        .map_err(refused)?
        .filter(|raw| !raw.is_empty())
        .ok_or_else(|| {
            CmdError::click(format!(
                "credential item {name:?} has no non-empty string field {field:?}"
            ))
        })?;
        println!("{raw}");
        return Ok(());
    }
    let value = match store {
        Store::Skarbiec(vault) => vault.read_item(name).await.map_err(|err| {
            CmdError::click(format!(
                "{err}; this store answers per field: name one with --field"
            ))
        })?,
        Store::File(_) => crate::credential_store::read_item(name)
            .await
            .map_err(refused)?,
    };
    if let Some(object) = value.as_object() {
        if let (Some(raw), [_]) = (
            object.get("value").and_then(Value::as_str),
            object.keys().collect::<Vec<_>>().as_slice(),
        ) {
            println!("{raw}");
            return Ok(());
        }
    }
    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(())
}

pub(crate) async fn ls(store: &Store, as_json: bool) -> Result<(), CmdError> {
    let stored = match store {
        Store::Skarbiec(vault) => vault.list_items().await,
        Store::File(path) => crate::credential_store::write::file_items(path),
    }
    .map_err(refused)?;
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

pub(crate) async fn rm(store: &Store, name: &str) -> Result<(), CmdError> {
    match store {
        Store::Skarbiec(vault) => vault.delete_item(name).await,
        Store::File(path) => {
            crate::credential_store::write::delete_item_at(
                &Backend::File { path: path.clone() },
                name,
            )
            .await
        }
    }
    .map_err(refused)?;
    println!("removed credential item {name:?}");
    Ok(())
}
