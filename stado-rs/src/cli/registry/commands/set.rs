//! `stado registry set` — change one field of the canonical registry under
//! the generation it was read at.
//!
//! The week of 2026-09-12 held six edits of the shape
//! `registry pull > ~/.oko/registry-pull.json`, `sed -i '' '309s|...|...|'`,
//! `registry validate ~/.oko/registry-pull.json`: the whole fleet document
//! pulled into a scratch file, one line rewritten by hand, the file validated
//! and thrown away. A line number is not a field, a scratch file is not the
//! registry, and nothing about that sequence survives to the next edit.
//!
//! This is the write half of [`pull --path`](super::pull): the same dotted
//! path names the field, the value replaces it, and the document goes back
//! under the generation the read returned, so a registry that moved in
//! between is refused rather than overwritten. The path must already exist —
//! a typo is a refusal naming what is there, not a new key nobody declared.

use serde_json::Value;

use crate::cli::registry::commands::pull::select;
use crate::cli::registry::write::conflict::RegistryWriteError;
use crate::cli::registry::write::document::{validate_for_write, warn_scoped_validation};
use crate::cli::registry::write::upload::upload_payload;
use crate::cli::CmdError;
use crate::targets::{self, RegistryStore};

mod path;

use path::{holder_of, leaf};

const SET_RECEIPT_SCHEMA: &str = "stado.registry-set-receipt.v1";
/// The directory whose readers count on its generation, and the field they
/// count.
const DIRECTORY_KEY: &str = "service_directory";
const DIRECTORY_GENERATION: &str = "generation";

/// The value to write: JSON when the argument parses as JSON, the argument
/// itself when it does not, so `--value 8789` is a number, `--value '"8789"'`
/// a string, and `--value /Users/charles/.stado` the path it looks like.
fn parsed(value: &str) -> Value {
    serde_json::from_str(value).unwrap_or_else(|_| Value::String(value.to_string()))
}

/// The consumer field only a Stado that knows it can read.
///
/// A host's resolver parses the service directory strictly: a consumer
/// carrying a field its build has never heard of makes the whole document
/// invalid for that host, and it resolves nothing at all. `grants` arrived in
/// 0.21.35, and on 2026-09-20 `charless-mac-mini` — the host every service
/// resolves through — still ran 0.21.32. Declaring one grant that morning
/// would have taken the fleet's resolution down, so this refuses the write
/// until the hosts can read it.
const GRANTS_FIELD: &str = "grants";
const GRANTS_SINCE: &str = "0.21.35";
const MANAGED_VERSIONS: &str = "managed_versions";
const STADO_BINARY: &str = "stado";

/// Hosts whose declared Stado is older than `GRANTS_SINCE`, or declares none.
fn hosts_behind(document: &Value) -> Vec<String> {
    let mut out = Vec::new();
    for target in document
        .get("targets")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let Some(name) = target.get("name").and_then(Value::as_str) else {
            continue;
        };
        let declared = target
            .get(MANAGED_VERSIONS)
            .and_then(|versions| versions.get(STADO_BINARY))
            .and_then(Value::as_str)
            .unwrap_or_default();
        if !crate::providers::local::disk_cleanup::catalogue::version_at_least(
            declared,
            GRANTS_SINCE,
        ) {
            out.push(match declared.is_empty() {
                true => format!("{name} (declares no stado version)"),
                false => format!("{name} ({declared})"),
            });
        }
    }
    out
}

/// Whether this path writes a consumer's declared grants.
fn declares_grants(path: &str) -> bool {
    path.split('.').any(|segment| segment == GRANTS_FIELD) && path.contains("consumers")
}

/// `stado registry set --path P --value V [--json]`.
pub async fn set(path: &str, value: &str, json_output: bool) -> Result<(), CmdError> {
    if path.trim().is_empty() {
        return Err(CmdError::usage(
            "--path names the field to change, as `registry pull --path` reads it",
        ));
    }
    let store = RegistryStore::open().await?;
    let blob = store.read_versioned().await?.ok_or_else(|| {
        CmdError::click(format!(
            "could not fetch registry from {}",
            store.location()
        ))
    })?;
    let mut document: Value = serde_json::from_str(&blob.content)?;
    if declares_grants(path) {
        let behind = hosts_behind(&document);
        if !behind.is_empty() {
            return Err(CmdError::click(format!(
                "declaring `{GRANTS_FIELD}` would make this registry unreadable for {} host(s) \
                 whose Stado is older than {GRANTS_SINCE}: {}. Their resolvers parse the service \
                 directory strictly and reject a consumer field they do not know, so they would \
                 resolve nothing at all. Bring them forward first — \
                 `stado release promote-version stado <version> --host <HOST>` then \
                 `stado release host-state --host <HOST> --binary stado --apply` — and check what \
                 each one actually runs with `stado release host-state --host <HOST> --binary \
                 stado`, because an installed binary can lag the version its registry entry \
                 declares.",
                behind.len(),
                behind.join(", ")
            )));
        }
    }
    // Read the field first, so a path that does not resolve is refused
    // before anything is serialised, with the reader's own sentence. A last
    // segment the document does not carry yet is not such a path: its
    // holder is there and the write creates it, so what it replaces is
    // nothing.
    let previous = match select(&document, path) {
        Ok(found) => found.clone(),
        Err(refusal) => match holder_of(&document, path) {
            Some(Value::Object(_)) => Value::Null,
            _ => return Err(refusal),
        },
    };
    let replacement = parsed(value);
    if previous == replacement {
        return report(
            json_output,
            "unchanged",
            path,
            &previous,
            &replacement,
            &blob.version,
            None,
            store.location(),
        );
    }
    *leaf(&mut document, path)? = replacement.clone();
    // A service directory that changed and kept its generation is a document
    // every resolver believes it has already read: the validator refuses it,
    // and rightly. The number belongs to the change, so it moves with it here
    // rather than in a second command an operator has to remember.
    if path.starts_with(DIRECTORY_KEY) {
        let generation = document
            .get(DIRECTORY_KEY)
            .and_then(|directory| directory.get(DIRECTORY_GENERATION))
            .and_then(Value::as_u64)
            .unwrap_or_default();
        if let Some(Value::Object(fields)) = document.get_mut(DIRECTORY_KEY) {
            fields.insert(
                DIRECTORY_GENERATION.to_string(),
                Value::from(generation.saturating_add(1)),
            );
        }
    }
    let payload = serde_json::to_string_pretty(&document)?;
    // The same gate `push` runs: a document that would not validate never
    // reaches the registry, whatever field was changed.
    warn_scoped_validation(validate_for_write(&document).await?);
    let location = targets::registry_location();
    // Always fenced by the generation this command read: the edit and the
    // read are one operation here, so there is no token for a caller to
    // carry and no window for a concurrent publication to be overwritten in.
    match upload_payload(&payload, false, false, Some(&blob.version)).await {
        Ok((generation, _previous_generation)) => report(
            json_output,
            "set",
            path,
            &previous,
            &replacement,
            &blob.version,
            Some(&generation),
            &location,
        ),
        Err(RegistryWriteError::Conflict(conflict)) => Err(conflict.error()),
        Err(RegistryWriteError::Failed(error)) => Err(error),
    }
}

#[allow(clippy::too_many_arguments)]
fn report(
    json_output: bool,
    state: &str,
    path: &str,
    previous: &Value,
    replacement: &Value,
    read_generation: &str,
    generation: Option<&str>,
    location: &str,
) -> Result<(), CmdError> {
    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "schema": SET_RECEIPT_SCHEMA,
                "state": state,
                "location": location,
                "path": path,
                "previous": previous,
                "value": replacement,
                "read_generation": read_generation,
                "generation": generation,
            }))?
        );
        return Ok(());
    }
    match generation {
        Some(generation) => println!(
            "set {path} -> {location} generation={generation}; was {}",
            compact(previous)
        ),
        None => println!(
            "{path} already holds {}; nothing written",
            compact(previous)
        ),
    }
    Ok(())
}

fn compact(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => serde_json::to_string(other).unwrap_or_default(),
    }
}
