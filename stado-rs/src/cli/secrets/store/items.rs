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
use crate::primitives::failure::FailureCode;
use crate::skarbiec::Client;

use crate::cli::secrets::store::resolve::{client, unknown};

/// Where `stado credentials get|put|ls|rm` act.
pub(crate) enum Store {
    File(PathBuf),
    Skarbiec(Client),
}

/// The selected store; a Skarbiec store is reached with the store
/// administrator's grant.
pub(crate) fn store() -> Result<Store, CmdError> {
    match crate::credential_store::selected().map_err(CmdError::from)? {
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

/// The payload on stdin and the kind it is stored as: the caller's
/// `--type`, else the payload's own `kind`, else `stado-secret`.
fn stdin_payload(item_type: Option<&str>) -> Result<(Value, String), CmdError> {
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
        .filter(|kind| !kind.trim().is_empty())
        .map(str::to_string);
    let item_kind = item_type
        .map(str::to_string)
        .or(declared)
        .unwrap_or_else(|| "stado-secret".to_string());
    Ok((value, item_kind))
}

pub(crate) async fn put(
    store: &Store,
    name: &str,
    item_type: Option<&str>,
) -> Result<(), CmdError> {
    let (value, item_kind) = stdin_payload(item_type)?;
    let item_kind = item_kind.as_str();
    match store {
        Store::Skarbiec(vault) => vault
            .write_item(name, item_kind, &value)
            .await
            .map_err(CmdError::from)?,
        Store::File(path) => crate::credential_store::write::write_item_at(
            &Backend::File { path: path.clone() },
            name,
            item_kind,
            &value,
            &Value::Null,
        )
        .await
        .map_err(CmdError::from)?,
    }
    println!("stored credential item {name:?} as {item_kind:?}");
    Ok(())
}

/// `put --role ROLE`: write the item that plays ROLE in the selected store.
/// The one holder is rewritten; with none, the item is created under a fresh
/// id and tagged `stado:role:<role>`, so a writer never names an item, not
/// even the first time.
pub(crate) async fn put_role(role: &str, item_type: Option<&str>) -> Result<(), CmdError> {
    let (value, item_kind) = stdin_payload(item_type)?;
    let item =
        crate::credential_store::write::write_role_item_with(role, &item_kind, &value, &json!({}))
            .await
            .map_err(CmdError::from)?;
    println!("stored the item playing role {role:?} ({item}) as {item_kind:?}");
    Ok(())
}

/// `put NAME --field F --route URL --consumer C --grant-file FILE`: replace one
/// field of an existing item under that consumer's own `rotate:NAME#F` grant,
/// keeping every other field. The value comes from stdin and never travels in
/// argv. A product that refreshes its own token writes it back this way
/// without owner authority.
pub(crate) async fn rotate(client: &Client, name: &str, field: &str) -> Result<(), CmdError> {
    let value = read_value_from_stdin()?;
    if value.is_empty() {
        return Err(CmdError::click(format!(
            "stdin was empty; pipe the new {name}#{field} value in"
        )));
    }
    let revision = client
        .rotate_field(name, field, &value)
        .await
        .map_err(CmdError::from)?;
    match revision {
        Some(revision) => println!("rotated {name:?} field {field:?} to revision {revision}"),
        None => println!("rotated {name:?} field {field:?}"),
    }
    Ok(())
}

pub(crate) async fn get(store: &Store, name: &str, field: Option<&str>) -> Result<(), CmdError> {
    if let Some(field) = field {
        let raw = match store {
            Store::Skarbiec(vault) => vault.read_declared_string(name, field).await,
            Store::File(_) => crate::credential_store::read_string(name, field).await,
        }
        .map_err(CmdError::from)?
        .filter(|raw| !raw.is_empty())
        .ok_or_else(|| {
            CmdError::click(format!(
                "credential item {name:?} has no non-empty string field {field:?}"
            ))
            .stating(FailureCode::NotFound)
        })?;
        println!("{raw}");
        return Ok(());
    }
    let value = match store {
        Store::Skarbiec(vault) => vault.read_item(name).await.map_err(|err| {
            let code = err.failure_code();
            CmdError::click(format!(
                "{err}; this store answers per field: name one with --field"
            ))
            .stating(code)
        })?,
        Store::File(_) => crate::credential_store::read_item(name)
            .await
            .map_err(CmdError::from)?,
    };
    if let Some(object) = value.as_object() {
        if let (Some(raw), [_]) = (
            object.get("value").and_then(Value::as_str),
            object.keys().collect::<Vec<_>>().as_slice(),
        ) {
            crate::skarbiec::envelope::plain(Some(raw.to_string())).map_err(|error| {
                let code = error.failure_code();
                CmdError::click(format!("credential item {name:?} field \"value\": {error}"))
                    .stating(code)
            })?;
            println!("{raw}");
            return Ok(());
        }
    }
    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(())
}

/// The id of the one live item in `store` carrying `stado:role:<role>`. The
/// refusal names the role and says whether no item or several carry it, so a
/// vault whose item lost its tag is told apart from one holding two.
pub(crate) async fn item_in_role(store: &Store, role: &str) -> Result<String, CmdError> {
    let stored = match store {
        Store::Skarbiec(vault) => vault.list_items().await,
        Store::File(path) => crate::credential_store::write::file_items(path),
    }
    .map_err(CmdError::from)?;
    crate::skarbiec::roles::item_for_role(&stored, role)
        .map(|item| item.id.clone())
        .map_err(|detail| CmdError::click(detail).stating(FailureCode::NotFound))
}

pub(crate) async fn ls(store: &Store, as_json: bool) -> Result<(), CmdError> {
    let stored = match store {
        Store::Skarbiec(vault) => vault.list_items().await,
        Store::File(path) => crate::credential_store::write::file_items(path),
    }
    .map_err(CmdError::from)?;
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
    .map_err(CmdError::from)?;
    println!("removed credential item {name:?}");
    Ok(())
}
