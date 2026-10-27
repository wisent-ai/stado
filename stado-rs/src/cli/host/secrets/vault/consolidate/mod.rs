//! Merge retired Stado grants into the product grant without copying a bearer
//! through the operator's machine. The target reads its existing bearer file.

pub(in crate::cli::host) mod revoke;

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
        .stating(crate::primitives::failure::FailureCode::InfraDown)
    })?;
    let mut capabilities = BTreeSet::new();
    let now = crate::cli::entry::error::epoch_seconds()?;
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
                .stating(crate::primitives::failure::FailureCode::NotFound)
            })?;
        let expires_at = grant
            .get("expires_at")
            .and_then(Value::as_u64)
            .ok_or_else(|| {
                CmdError::click(format!(
                    "{}: {consumer} has no numeric expiry; no grant was changed",
                    target.name
                ))
                .stating(crate::primitives::failure::FailureCode::InfraDown)
            })?;
        if expires_at <= now {
            return Err(CmdError::refused(format!(
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
                .stating(crate::primitives::failure::FailureCode::InfraDown)
            })?;
        for entry in entries {
            let action = entry.get("action").and_then(Value::as_str).ok_or_else(|| {
                CmdError::click(format!(
                    "{}: {consumer} has a malformed action",
                    target.name
                ))
                .stating(crate::primitives::failure::FailureCode::InfraDown)
            })?;
            let item = entry.get("item").and_then(Value::as_str).ok_or_else(|| {
                CmdError::click(format!("{}: {consumer} has a malformed item", target.name))
                    .stating(crate::primitives::failure::FailureCode::InfraDown)
            })?;
            let capability = match entry.get("field").and_then(Value::as_str) {
                Some(field) => format!("{action}:{item}#{field}"),
                None => format!("{action}:{item}"),
            };
            capabilities.insert(capability);
        }
    }
    if capabilities.is_empty() {
        return Err(CmdError::refused(format!(
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
            .stating(crate::primitives::failure::FailureCode::NotFound)
        })?;
    // A capability without its item or action would ask Skarbiec about empty
    // words and read its answer as the bearer's verdict.
    let named = |key: &str| {
        first
            .get(key)
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                CmdError::refused(format!(
                    "{}: stado's first capability names no {key}, so its bearer cannot be verified: {first}; no grant was changed",
                    target.name
                ))
            })
    };
    let mut probe = vec![
        "grant".into(),
        "verify".into(),
        "stado".into(),
        named("item")?.into(),
        "--action".into(),
        named("action")?.into(),
        "--token-file".into(),
        token_file.into(),
    ];
    if let Some(field) = first.get("field").and_then(Value::as_str) {
        probe.extend(["--field".into(), field.into()]);
    }
    let (_, verdict) = remote_skarbiec_json(host, &probe).await?;
    if verdict.get("allowed").and_then(Value::as_bool) != Some(true) {
        return Err(CmdError::refused(format!(
            "{}: stado bearer verification refused: {verdict}; no grant was changed",
            target.name
        )));
    }
    // Keep the shortest source lifetime so consolidation cannot revive or
    // extend a retired consumer's authority: the merged grant lives until
    // revoked only when every source does, and otherwise ends with the
    // earliest source, measured from the clock read just before the issue.
    let issue_time = crate::cli::entry::error::epoch_seconds()?;
    let Some(lifetime) =
        crate::credential_store::grant::GrantLifetime::left(earliest_expiry, issue_time)
    else {
        return Err(CmdError::refused(format!(
            "{}: a source grant expires before consolidation can finish; no grant was changed",
            target.name
        )));
    };
    let audience = audience.ok_or_else(|| {
        CmdError::click(format!("{}: stado grant has no audience", target.name))
            .stating(crate::primitives::failure::FailureCode::InfraDown)
    })?;
    let merged = capabilities.iter().cloned().collect::<Vec<_>>().join(",");
    let mut issue: Vec<String> = vec![
        "grant".into(),
        "issue".into(),
        "stado".into(),
        "--capabilities".into(),
        merged,
        "--token-file".into(),
        token_file.into(),
    ];
    issue.extend(lifetime.args());
    issue.extend([
        "--audience".into(),
        audience,
        "--replace-capabilities".into(),
    ]);
    // Which actions may share one grant with field reads is Skarbiec's rule:
    // its refusal names the capability, and nothing has been changed yet.
    remote_skarbiec_json(host, &issue).await.map_err(|error| {
        error.within(format!(
            "{}: Skarbiec did not issue the merged grant; no grant was changed",
            target.name
        ))
    })?;
    let (_, final_listing) = remote_skarbiec_json(host, &["grant".into(), "list".into()]).await?;
    let issued = final_listing
        .as_array()
        .and_then(|rows| rows.iter().find(|row| row["consumer"] == "stado"))
        .ok_or_else(|| {
            CmdError::click(format!(
                "{}: issued Stado grant was not readable",
                target.name
            ))
            .stating(crate::primitives::failure::FailureCode::InfraDown)
        })?;
    // Skarbiec counts the lifetime from the moment it issues, after this
    // call's own round trip, so the issued expiry is read back and compared
    // with the earliest source's instead of being assumed.
    if let Some(issued_expiry) = issued["expires_at"].as_u64() {
        if issued_expiry > earliest_expiry {
            return Err(CmdError::click(format!(
                "{}: the issued Stado grant expires at {issued_expiry}, after the earliest source grant at {earliest_expiry}; \
                 the merged grant outlives a retired consumer's authority and must be re-issued",
                target.name
            ))
            .stating(crate::primitives::failure::FailureCode::InfraDown));
        }
    }
    let recorded = issued["capabilities"].as_array().ok_or_else(|| {
        CmdError::click(format!(
            "{}: issued Stado grant carries no capability array",
            target.name
        ))
        .stating(crate::primitives::failure::FailureCode::InfraDown)
    })?;
    let unreadable = |what: &str, entry: &Value| {
        CmdError::click(format!(
            "{}: an entry of the issued Stado grant names no {what}: {entry}",
            target.name
        ))
        .stating(crate::primitives::failure::FailureCode::InfraDown)
    };
    let mut actual: BTreeSet<String> = BTreeSet::new();
    for entry in recorded.iter() {
        let action = entry["action"]
            .as_str()
            .ok_or_else(|| unreadable("action", entry))?;
        let item = entry["item"]
            .as_str()
            .ok_or_else(|| unreadable("item", entry))?;
        actual.insert(match entry["field"].as_str() {
            Some(field) => format!("{action}:{item}#{field}"),
            None => format!("{action}:{item}"),
        });
    }
    if actual != capabilities {
        return Err(CmdError::click(format!("{}: issued Stado grant differs from the requested capabilities: actual={actual:?}, requested={capabilities:?}", target.name)).stating(crate::primitives::failure::FailureCode::InfraDown));
    }
    let report = json!({"host": target.name, "consumer": "stado", "token_file": token_file, "capabilities": capabilities, "merged_from": sources, "retired_grants_preserved": true});
    if json_output {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!("{}: stado grant now holds {} capabilities from {} retired grants; verify readers before revoking old grants", target.name, recorded.len(), sources.len());
    }
    Ok(())
}
