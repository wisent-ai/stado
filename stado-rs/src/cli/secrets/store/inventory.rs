//! Nonsecret item metadata: one local vault file, or the vault a registry
//! host holds. Names, kinds, states and tags only, never a field value.

use std::os::unix::fs::MetadataExt;

use serde_json::{json, Value};

use crate::cli::{table, CmdError};

use crate::cli::secrets::store::resolve::skarbiec_launcher;

/// Nonsecret item metadata from one local vault file.
///
/// Read through the launcher because the launcher holds the unlock, and with
/// `--all` on purpose: a trashed id still occupies its name, so an inventory
/// that hid the trash would let a merge call an occupied id absent and write
/// over it.
pub(crate) fn vault_items(
    launcher: &std::path::Path,
    vault: &std::path::Path,
) -> Result<Vec<Value>, CmdError> {
    let output = std::process::Command::new(launcher)
        .arg("list")
        .arg("--all")
        .env("SKARBIEC_VAULT_FILE", vault)
        .output()?;
    if !output.status.success() {
        return Err(CmdError::click(format!(
            "{} could not inspect {}: {}",
            launcher.display(),
            vault.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    serde_json::from_slice(&output.stdout)
        .map_err(|_| CmdError::click("Skarbiec inventory was not a JSON array"))
}

/// `credentials inspect-vault --host` — item names on the host that holds them.
///
/// The remote read is `skarbiec list`, the same read-only subcommand
/// `fleet vaults` already runs on a host to count its vaults, addressed
/// through [`crate::deploy::host_capability`] so the binary, the vault and
/// GNUPGHOME are the host's own. An item's NAME is its `id`; nothing here
/// reads or prints a field value.
pub(crate) async fn inspect_host_vault(
    host: &str,
    matching: Option<&str>,
    json: bool,
) -> Result<(), CmdError> {
    let resolved = crate::deploy::host_channel::canonical_target(host)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let runner = crate::deploy::production_runner();
    let broker = crate::deploy::host_capability::resolve(&resolved, &Default::default(), &runner)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let inventory = crate::deploy::host_capability::items(&resolved, &broker, &runner)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;

    let mut rows: Vec<(String, String, String, Vec<String>)> = inventory
        .iter()
        .filter_map(|item| {
            let name = item.get("id").and_then(Value::as_str)?.to_string();
            if let Some(text) = matching {
                if !name.to_lowercase().contains(&text.to_lowercase()) {
                    return None;
                }
            }
            let tags = item
                .get("tags")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect();
            Some((
                name,
                item.get("kind")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                item.get("state")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                tags,
            ))
        })
        .collect();
    rows.sort();

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "host": resolved.name,
                "vault": broker.vault,
                "items_total": inventory.len(),
                "matched": rows.len(),
                "items": rows
                    .iter()
                    .map(|(name, kind, state, tags)| json!({
                        "name": name,
                        "kind": kind,
                        "state": state,
                        "tags": tags,
                    }))
                    .collect::<Vec<Value>>(),
            }))?
        );
        return Ok(());
    }
    println!("host:      {}", resolved.name);
    println!("vault:     {}", broker.vault);
    println!("items:     {} total, {} shown", inventory.len(), rows.len());
    for (name, kind, state, tags) in &rows {
        println!("  {name:<52} {kind:<12} {state:<8} {}", tags.join(","));
    }
    Ok(())
}

pub(crate) fn inspect_vault(
    path: &str,
    matching: Option<&str>,
    json: bool,
) -> Result<(), CmdError> {
    let metadata = std::fs::symlink_metadata(path)?;
    let unsafe_bits = u32::from_str_radix("077", u8::BITS).unwrap_or_default();
    if !metadata.is_file()
        || metadata.file_type().is_symlink()
        || metadata.mode() & unsafe_bits != u32::default()
    {
        return Err(CmdError::click(
            "vault must be an owner-only regular local file",
        ));
    }
    let launcher = skarbiec_launcher()?;
    let mut items = vault_items(&launcher, std::path::Path::new(path))?;
    if let Some(text) = matching {
        let needle = text.to_lowercase();
        items.retain(|item| {
            item.get("id")
                .and_then(Value::as_str)
                .is_some_and(|name| name.to_lowercase().contains(&needle))
        });
    }
    let grants_output = std::process::Command::new(&launcher)
        .args(["grant", "list"])
        .env("SKARBIEC_VAULT_FILE", path)
        .output()?;
    if !grants_output.status.success() {
        return Err(CmdError::click(format!(
            "{} could not inspect grants in {}: {}",
            launcher.display(),
            path,
            String::from_utf8_lossy(&grants_output.stderr).trim()
        )));
    }
    let grants: Vec<Value> = serde_json::from_slice(&grants_output.stdout)
        .map_err(|_| CmdError::click("Skarbiec grant inventory was not a JSON array"))?;
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "vault": path,
                "items": items,
                "count": items.len(),
                "grants": grants,
            }))?
        );
        return Ok(());
    }
    let rows = items
        .iter()
        .map(|item| {
            vec![
                item.get("id")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                item.get("type")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                item.get("updated_at")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                item.get("deleted")
                    .and_then(Value::as_bool)
                    .unwrap_or_default()
                    .to_string(),
            ]
        })
        .collect::<Vec<Vec<String>>>();
    table::print(&["NAME", "TYPE", "UPDATED", "DELETED"], &rows);
    println!(
        "{} item(s), {} grant(s) in {}",
        items.len(),
        grants.len(),
        path
    );
    Ok(())
}
