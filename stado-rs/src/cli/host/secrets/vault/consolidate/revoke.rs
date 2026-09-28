//! Revoking a retired grant once the `stado` grant covers it.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::cli::host::secrets::vault::mirror::remote_skarbiec_json;
use crate::cli::CmdError;

/// Revoke a retired consumer's grant on HOST once Stado holds everything it
/// could read, so one identity is left instead of one per role.
///
/// A capability on an item the vault no longer holds (a role-named item that
/// was renamed to its product) opens nothing, so it does not keep the retired
/// grant alive.
///
/// Refused for `stado` itself, and refused while the retired grant still
/// carries a capability the `stado` grant lacks: `consolidate` first, then
/// revoke, so no reader loses access in between.
pub async fn revoke_retired(host: &str, consumer: &str, json_output: bool) -> Result<(), CmdError> {
    if consumer == "stado" {
        return Err(CmdError::usage(
            "stado is the identity that remains; it is never revoked",
        ));
    }
    let (target, listing) = remote_skarbiec_json(host, &["grant".into(), "list".into()]).await?;
    let rows = listing.as_array().ok_or_else(|| {
        CmdError::click(format!(
            "{}: Skarbiec grant list did not answer an array",
            target.name
        ))
    })?;
    let capabilities_of = |name: &str| -> Option<BTreeSet<String>> {
        let grant = rows.iter().find(|row| row["consumer"] == name)?;
        Some(
            grant["capabilities"]
                .as_array()?
                .iter()
                .map(|entry| {
                    format!(
                        "{}:{}#{}",
                        entry["action"].as_str().unwrap_or_default(),
                        entry["item"].as_str().unwrap_or_default(),
                        entry["field"].as_str().unwrap_or_default()
                    )
                })
                .collect(),
        )
    };
    let retired = capabilities_of(consumer).ok_or_else(|| {
        CmdError::click(format!(
            "{}: no grant for {consumer}; nothing to revoke",
            target.name
        ))
    })?;
    let stado = capabilities_of("stado").ok_or_else(|| {
        CmdError::click(format!(
            "{}: no grant for stado; consolidate before revoking",
            target.name
        ))
    })?;
    let (_, items) = remote_skarbiec_json(host, &["list".into()]).await?;
    let held: BTreeSet<&str> = items
        .as_array()
        .into_iter()
        .flatten()
        .filter(|row| row.get("deleted").and_then(Value::as_bool) != Some(true))
        .filter_map(|row| row.get("id").and_then(Value::as_str))
        .collect();
    let item_of = |capability: &str| -> String {
        let rest = capability.split_once(':').map_or("", |(_, rest)| rest);
        rest.rsplit_once('#')
            .map_or(rest, |(item, _)| item)
            .to_string()
    };
    let missing: Vec<&String> = retired
        .difference(&stado)
        .filter(|capability| held.contains(item_of(capability).as_str()))
        .collect();
    if !missing.is_empty() {
        return Err(CmdError::click(format!(
            "{}: stado does not hold {missing:?} that {consumer} can read; run `stado credentials grant consolidate --host {} --from {consumer} --token-file <stado bearer>` first",
            target.name, target.name
        )));
    }
    remote_skarbiec_json(
        host,
        &["grant".into(), "revoke".into(), consumer.to_string()],
    )
    .await?;
    let report = json!({"host": target.name, "revoked": consumer, "covered_by": "stado", "capabilities": retired});
    if json_output {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!(
            "{}: revoked {consumer}; stado holds all {} of its capabilities",
            target.name,
            retired.len()
        );
    }
    Ok(())
}
