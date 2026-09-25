//! Merge retired Stado grants into the product grant without copying a bearer
//! through the operator's machine. The target reads its existing bearer file.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::cli::host::secrets::vault::mirror::remote_skarbiec_json;
use crate::cli::CmdError;

pub async fn consolidate(
    host: &str,
    sources: &[String],
    token_file: &str,
    json_output: bool,
) -> Result<(), CmdError> {
    if sources.is_empty() {
        return Err(CmdError::usage(
            "name at least one retired consumer with --from",
        ));
    }
    if sources.iter().any(|source| source == "stado") {
        return Err(CmdError::usage("--from names retired consumers, not stado"));
    }
    if !token_file.starts_with('/') {
        return Err(CmdError::usage(
            "--token-file must be an absolute path on the vault host",
        ));
    }
    let (target, listing) = remote_skarbiec_json(host, &["grant".into(), "list".into()]).await?;
    let grants = listing.as_array().ok_or_else(|| {
        CmdError::click(format!(
            "{}: Skarbiec grant list was not an array",
            target.name
        ))
    })?;
    let mut capabilities = BTreeSet::new();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|error| CmdError::click(error.to_string()))?
        .as_secs();
    let mut earliest_expiry = u64::MAX;
    let mut audience = None;
    for consumer in std::iter::once("stado").chain(sources.iter().map(String::as_str)) {
        let grant = grants
            .iter()
            .find(|entry| entry.get("consumer").and_then(Value::as_str) == Some(consumer))
            .ok_or_else(|| {
                CmdError::click(format!(
                    "{}: no grant for {consumer}; no grant was changed",
                    target.name
                ))
            })?;
        let expires_at = grant
            .get("expires_at")
            .and_then(Value::as_u64)
            .ok_or_else(|| {
                CmdError::click(format!(
                    "{}: {consumer} has no numeric expiry; no grant was changed",
                    target.name
                ))
            })?;
        if expires_at <= now {
            return Err(CmdError::click(format!(
                "{}: {consumer} has expired; no grant was changed",
                target.name
            )));
        }
        earliest_expiry = earliest_expiry.min(expires_at);
        if consumer == "stado" {
            audience = grant
                .get("audience")
                .and_then(Value::as_str)
                .map(str::to_string);
        }
        let entries = grant
            .get("capabilities")
            .and_then(Value::as_array)
            .ok_or_else(|| {
                CmdError::click(format!(
                    "{}: {consumer} has no capability array; no grant was changed",
                    target.name
                ))
            })?;
        for entry in entries {
            let action = entry.get("action").and_then(Value::as_str).ok_or_else(|| {
                CmdError::click(format!(
                    "{}: {consumer} has a malformed action",
                    target.name
                ))
            })?;
            let item = entry.get("item").and_then(Value::as_str).ok_or_else(|| {
                CmdError::click(format!("{}: {consumer} has a malformed item", target.name))
            })?;
            if matches!(action, "acquire" | "lifecycle") {
                return Err(CmdError::click(format!("{}: {consumer} has {action}:{item}; Skarbiec forbids merging this grant with field reads", target.name)));
            }
            let capability = match entry.get("field").and_then(Value::as_str) {
                Some(field) => format!("{action}:{item}#{field}"),
                None => format!("{action}:{item}"),
            };
            capabilities.insert(capability);
        }
    }
    if capabilities.is_empty() {
        return Err(CmdError::click(format!(
            "{}: grants contain no capabilities; no grant was changed",
            target.name
        )));
    }
    // Probe the bearer before replacement. This is a read-only verify, unlike
    // ensure, so an invalid file cannot silently become the new Stado bearer.
    let first = grants
        .iter()
        .find(|entry| entry.get("consumer").and_then(Value::as_str) == Some("stado"))
        .and_then(|grant| grant.get("capabilities"))
        .and_then(Value::as_array)
        .and_then(|entries| entries.first())
        .ok_or_else(|| {
            CmdError::click(format!(
                "{}: stado has no capability to verify its bearer",
                target.name
            ))
        })?;
    let mut probe = vec![
        "grant".into(),
        "verify".into(),
        "stado".into(),
        first["item"].as_str().unwrap_or_default().into(),
        "--action".into(),
        first["action"].as_str().unwrap_or_default().into(),
        "--token-file".into(),
        token_file.into(),
    ];
    if let Some(field) = first.get("field").and_then(Value::as_str) {
        probe.extend(["--field".into(), field.into()]);
    }
    let (_, verdict) = remote_skarbiec_json(host, &probe).await?;
    if verdict.get("allowed").and_then(Value::as_bool) != Some(true) {
        return Err(CmdError::click(format!(
            "{}: stado bearer verification refused: {verdict}; no grant was changed",
            target.name
        )));
    }
    // `grant issue` renews for thirty days unless the remaining lifetime is
    // explicit. Keep the shortest source lifetime so consolidation cannot
    // revive or extend a retired consumer's authority.
    let remaining = earliest_expiry.saturating_sub(now);
    if remaining <= 1 {
        return Err(CmdError::click(format!(
            "{}: a source grant expires before consolidation can finish; no grant was changed",
            target.name
        )));
    }
    let ttl = (remaining - 1).to_string();
    let audience = audience
        .ok_or_else(|| CmdError::click(format!("{}: stado grant has no audience", target.name)))?;
    let merged = capabilities.iter().cloned().collect::<Vec<_>>().join(",");
    remote_skarbiec_json(
        host,
        &[
            "grant".into(),
            "issue".into(),
            "stado".into(),
            "--capabilities".into(),
            merged,
            "--token-file".into(),
            token_file.into(),
            "--ttl-seconds".into(),
            ttl,
            "--audience".into(),
            audience,
            "--replace-capabilities".into(),
        ],
    )
    .await?;
    let (_, final_listing) = remote_skarbiec_json(host, &["grant".into(), "list".into()]).await?;
    let recorded = final_listing
        .as_array()
        .and_then(|rows| rows.iter().find(|row| row["consumer"] == "stado"))
        .and_then(|grant| grant["capabilities"].as_array())
        .ok_or_else(|| {
            CmdError::click(format!(
                "{}: issued Stado grant was not readable",
                target.name
            ))
        })?;
    let actual: BTreeSet<String> = recorded
        .iter()
        .map(|entry| {
            let action = entry["action"].as_str().unwrap_or_default();
            let item = entry["item"].as_str().unwrap_or_default();
            match entry["field"].as_str() {
                Some(field) => format!("{action}:{item}#{field}"),
                None => format!("{action}:{item}"),
            }
        })
        .collect();
    if actual != capabilities {
        return Err(CmdError::click(format!("{}: issued Stado grant differs from the requested capabilities: actual={actual:?}, requested={capabilities:?}", target.name)));
    }
    let report = json!({"host": target.name, "consumer": "stado", "token_file": token_file, "capabilities": capabilities, "merged_from": sources, "retired_grants_preserved": true});
    if json_output {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!("{}: stado grant now holds {} capabilities from {} retired grants; verify readers before revoking old grants", target.name, recorded.len(), sources.len());
    }
    Ok(())
}

/// Revoke a retired consumer's grant on HOST once Stado holds everything it
/// A capability on an item the vault no longer holds (a role-named item that
/// was renamed to its product) opens nothing, so it does not keep the retired
/// grant alive.
/// could read, so one identity is left instead of one per role.
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
    let missing: Vec<&String> = retired.difference(&stado).collect();
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
