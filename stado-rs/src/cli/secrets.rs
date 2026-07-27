//! `stado secrets` — operator surface for the separate Skarbiec service.
//!
//! Secret values travel in request bodies, never argv. Skarbiec performs
//! encryption, authorization, versioning, recovery-recipient handling, and
//! audit logging. Stado retains no local or cloud-secret-manager fallback.

use std::io::Read;

use clap::Subcommand;
use serde_json::{json, Value};

use super::{table, CmdError};

#[derive(Subcommand)]
pub enum SecretsCommands {
    /// Store an item in Skarbiec, reading its value from STDIN.
    Put {
        /// Skarbiec item id.
        name: String,
    },
    /// Print one Skarbiec item value or one exact string field to stdout.
    Get {
        /// Skarbiec item id.
        name: String,
        /// Print only this string field. The item id and field remain separate.
        #[arg(long)]
        field: Option<String>,
    },
    /// List metadata for items authorized by the current grant.
    Ls {
        /// Emit JSON instead of a table.
        #[arg(long)]
        json: bool,
    },
    /// Soft-delete a Skarbiec item.
    Rm {
        /// Skarbiec item id.
        name: String,
    },
}

pub async fn dispatch(command: SecretsCommands) -> Result<(), CmdError> {
    let vault =
        crate::skarbiec::Client::configured().map_err(|err| CmdError::click(err.to_string()))?;
    match command {
        SecretsCommands::Put { name } => put(&vault, &name).await,
        SecretsCommands::Get { name, field } => get(&vault, &name, field.as_deref()).await,
        SecretsCommands::Ls { json } => ls(&vault, json).await,
        SecretsCommands::Rm { name } => rm(&vault, &name).await,
    }
}

fn read_value_from_stdin() -> Result<String, CmdError> {
    let mut value = String::new();
    std::io::stdin().read_to_string(&mut value)?;
    let value = value.strip_suffix('\n').unwrap_or(&value);
    Ok(value.strip_suffix('\r').unwrap_or(value).to_string())
}

async fn put(vault: &crate::skarbiec::Client, name: &str) -> Result<(), CmdError> {
    let input = read_value_from_stdin()?;
    if input.is_empty() {
        return Err(CmdError::click(
            "stdin was empty; pipe the value in (stado secrets put NAME < file)",
        ));
    }
    let value = serde_json::from_str(&input).unwrap_or_else(|_| json!({"value": input}));
    vault
        .write_item(name, "stado-secret", &value)
        .await
        .map_err(|err| CmdError::click(err.to_string()))?;
    println!("stored Skarbiec item {name:?}");
    Ok(())
}

async fn get(
    vault: &crate::skarbiec::Client,
    name: &str,
    field: Option<&str>,
) -> Result<(), CmdError> {
    let value = vault
        .read_item(name)
        .await
        .map_err(|err| CmdError::click(err.to_string()))?;
    if let Some(field) = field {
        let raw = value
            .as_object()
            .and_then(|object| object.get(field))
            .and_then(Value::as_str)
            .filter(|raw| !raw.is_empty())
            .ok_or_else(|| {
                CmdError::click(format!(
                    "Skarbiec item {name:?} has no non-empty string field {field:?}"
                ))
            })?;
        println!("{raw}");
        return Ok(());
    }
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

async fn ls(vault: &crate::skarbiec::Client, as_json: bool) -> Result<(), CmdError> {
    let stored = vault
        .list_items()
        .await
        .map_err(|err| CmdError::click(err.to_string()))?;
    if as_json {
        println!("{}", serde_json::to_string_pretty(&stored)?);
        return Ok(());
    }
    if stored.is_empty() {
        println!("No Skarbiec items are visible to this grant.");
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

fn unknown() -> String {
    "-".to_string()
}

async fn rm(vault: &crate::skarbiec::Client, name: &str) -> Result<(), CmdError> {
    vault
        .delete_item(name)
        .await
        .map_err(|err| CmdError::click(err.to_string()))?;
    println!("soft-deleted Skarbiec item {name:?}");
    Ok(())
}
