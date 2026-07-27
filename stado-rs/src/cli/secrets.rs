//! `stado secrets` — operator surface for Azure Key Vault.
//!
//! Values are never stored in queue blobs, local files, process arguments, or
//! another cloud's secret manager. Vault authentication uses Managed Identity
//! or the current Azure CLI identity. `get` is the only command that renders a
//! value; `ls` reads metadata only.

use std::io::Read;

use clap::Subcommand;

use crate::azure_key_vault;

use super::{table, CmdError};

#[derive(Subcommand)]
pub enum SecretsCommands {
    /// Store a secret in Azure Key Vault, reading the value from STDIN.
    ///
    /// There is no --value flag: argv is visible in process listings and shell
    /// history. Pipe the value instead: `stado secrets put NAME < file`.
    Put {
        /// Key Vault secret name; letters, digits, and '-' only.
        name: String,
    },
    /// Print one Azure Key Vault secret value to stdout.
    Get {
        /// Secret name.
        name: String,
    },
    /// List Azure Key Vault secret metadata without downloading values.
    Ls {
        /// Emit JSON instead of a table.
        #[arg(long)]
        json: bool,
    },
    /// Soft-delete an Azure Key Vault secret.
    Rm {
        /// Secret name.
        name: String,
    },
}

pub async fn dispatch(command: SecretsCommands) -> Result<(), CmdError> {
    let client = reqwest::Client::new();
    let vault_url = crate::config::azure_key_vault_url();
    match command {
        SecretsCommands::Put { name } => put(&client, vault_url, &name).await,
        SecretsCommands::Get { name } => get(&client, vault_url, &name).await,
        SecretsCommands::Ls { json } => ls(&client, vault_url, json).await,
        SecretsCommands::Rm { name } => rm(&client, vault_url, &name).await,
    }
}

/// Read stdin as the secret value, dropping exactly one trailing line ending.
fn read_value_from_stdin() -> Result<String, CmdError> {
    let mut value = String::new();
    std::io::stdin().read_to_string(&mut value)?;
    let value = value.strip_suffix('\n').unwrap_or(&value);
    Ok(value.strip_suffix('\r').unwrap_or(value).to_string())
}

async fn put(client: &reqwest::Client, vault_url: &str, name: &str) -> Result<(), CmdError> {
    let value = read_value_from_stdin()?;
    if value.is_empty() {
        return Err(CmdError::click(
            "stdin was empty; pipe the secret in (stado secrets put NAME < file)",
        ));
    }
    azure_key_vault::write_secret(client, vault_url, name, &value)
        .await
        .map_err(|err| CmdError::click(err.to_string()))?;
    println!("stored secret {name:?} in Azure Key Vault {vault_url}");
    Ok(())
}

async fn get(client: &reqwest::Client, vault_url: &str, name: &str) -> Result<(), CmdError> {
    let value = azure_key_vault::read_secret(client, vault_url, name)
        .await
        .map_err(|err| CmdError::click(err.to_string()))?
        .ok_or_else(|| CmdError::click(format!("no Key Vault secret named {name:?}")))?;
    println!("{value}");
    Ok(())
}

async fn ls(client: &reqwest::Client, vault_url: &str, as_json: bool) -> Result<(), CmdError> {
    let stored = azure_key_vault::list_secrets(client, vault_url)
        .await
        .map_err(|err| CmdError::click(err.to_string()))?;
    if as_json {
        println!("{}", serde_json::to_string_pretty(&stored)?);
        return Ok(());
    }
    if stored.is_empty() {
        println!("No secrets stored in Azure Key Vault {vault_url}.");
        return Ok(());
    }
    let rows: Vec<Vec<String>> = stored
        .iter()
        .map(|secret| {
            vec![
                secret.name.clone(),
                secret
                    .updated
                    .map(|at| at.to_rfc3339())
                    .unwrap_or_else(unknown),
                secret
                    .enabled
                    .map(|enabled| enabled.to_string())
                    .unwrap_or_else(unknown),
            ]
        })
        .collect();
    table::print(&["NAME", "UPDATED", "ENABLED"], &rows);
    Ok(())
}

fn unknown() -> String {
    "-".to_string()
}

async fn rm(client: &reqwest::Client, vault_url: &str, name: &str) -> Result<(), CmdError> {
    if !azure_key_vault::delete_secret(client, vault_url, name)
        .await
        .map_err(|err| CmdError::click(err.to_string()))?
    {
        return Err(CmdError::click(format!(
            "no Key Vault secret named {name:?}"
        )));
    }
    println!("soft-deleted secret {name:?} from Azure Key Vault {vault_url}");
    Ok(())
}
