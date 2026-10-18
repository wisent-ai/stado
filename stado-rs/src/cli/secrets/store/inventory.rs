//! Nonsecret item metadata: one local vault file, or the vault a registry
//! host holds. Names, kinds, states and tags only, never a field value.

use std::os::unix::fs::MetadataExt;

use serde_json::{json, Value};

use crate::cli::{reporting::table, CmdError};

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
    let output = crate::wait::output(
        std::process::Command::new(launcher)
            .arg("list")
            .arg("--all")
            .env("SKARBIEC_VAULT_FILE", vault),
    )?;
    if !output.status.success() {
        return Err(CmdError::click(format!(
            "{} could not inspect {}: {}",
            launcher.display(),
            vault.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        ))
        .stating(crate::primitives::failure::FailureCode::InfraDown));
    }
    serde_json::from_slice(&output.stdout).map_err(|error| {
        CmdError::unreachable(format!("Skarbiec inventory was not a JSON array: {error}"))
    })
}

/// `credentials vault items --host` — item names on the host that holds them.
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
    let resolved = crate::cli::canonical_host(host).await?;
    let runner = crate::deploy::production_runner();
    let broker = crate::deploy::host_capability::resolve(&resolved, &Default::default(), &runner)
        .await
        .map_err(CmdError::from)?;
    let inventory = crate::deploy::host_capability::items(&resolved, &broker, &runner)
        .await
        .map_err(CmdError::from)?;

    // A field the host's listing does not carry is null in JSON and `not
    // reported` in text, not an empty kind, state or tag list; an item with
    // no id is counted as unnamed instead of vanishing from the listing.
    type Row = (String, Option<String>, Option<String>, Option<Vec<String>>);
    let text = |item: &Value, key: &str| item.get(key).and_then(Value::as_str).map(str::to_string);
    let mut unnamed: Vec<&Value> = Vec::new();
    let mut rows: Vec<Row> = Vec::new();
    for item in &inventory {
        let Some(name) = text(item, "id") else {
            unnamed.push(item);
            continue;
        };
        if let Some(text) = matching {
            if !name.to_lowercase().contains(&text.to_lowercase()) {
                continue;
            }
        }
        let tags = item.get("tags").and_then(Value::as_array).map(|tags| {
            tags.iter()
                .map(|tag| match tag.as_str() {
                    Some(tag) => tag.to_string(),
                    None => tag.to_string(),
                })
                .collect()
        });
        rows.push((name, text(item, "kind"), text(item, "state"), tags));
    }
    rows.sort();

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "host": resolved.name,
                "vault": broker.vault,
                "items_total": inventory.len(),
                "matched": rows.len(),
                "unnamed": unnamed,
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
    let shown = |value: &Option<String>| match value {
        Some(value) => value.clone(),
        None => "not reported".to_string(),
    };
    println!("host:      {}", resolved.name);
    println!("vault:     {}", broker.vault);
    println!("items:     {} total, {} shown", inventory.len(), rows.len());
    for (name, kind, state, tags) in &rows {
        let tags = match tags {
            Some(tags) => tags.join(","),
            None => "tags not reported".to_string(),
        };
        println!(
            "  {name:<52} {:<12} {:<8} {tags}",
            shown(kind),
            shown(state)
        );
    }
    for item in &unnamed {
        println!("  an item the host listed without an id: {item}");
    }
    Ok(())
}

pub(crate) fn inspect_vault(
    path: &str,
    matching: Option<&str>,
    json: bool,
) -> Result<(), CmdError> {
    let metadata = std::fs::symlink_metadata(path)?;
    if !metadata.is_file()
        || metadata.file_type().is_symlink()
        || crate::primitives::file_mode::open_to_others(metadata.mode())
    {
        return Err(CmdError::refused(
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
    let grants_output = crate::wait::output(
        std::process::Command::new(&launcher)
            .args(["grant", "list"])
            .env("SKARBIEC_VAULT_FILE", path),
    )?;
    if !grants_output.status.success() {
        return Err(CmdError::click(format!(
            "{} could not inspect grants in {}: {}",
            launcher.display(),
            path,
            String::from_utf8_lossy(&grants_output.stderr).trim()
        ))
        .stating(crate::primitives::failure::FailureCode::InfraDown));
    }
    let grants: Vec<Value> = serde_json::from_slice(&grants_output.stdout).map_err(|error| {
        CmdError::click(format!(
            "Skarbiec grant inventory was not a JSON array: {error}"
        ))
        .stating(crate::primitives::failure::FailureCode::InfraDown)
    })?;
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
    // A field an item does not carry reads `not reported`: an empty name or
    // type, or `false` for an unstated deletion, would read as Skarbiec's word.
    let rows = items
        .iter()
        .map(|item| {
            ["id", "type", "updated_at", "deleted"]
                .iter()
                .map(|key| crate::cli::reporting::field(&item[*key]))
                .collect()
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
