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

use crate::cli::registry::commands::pull::{kind, select};
use crate::cli::registry::write::conflict::RegistryWriteError;
use crate::cli::registry::write::document::{validate_for_write, warn_scoped_validation};
use crate::cli::registry::write::upload::upload_payload;
use crate::cli::CmdError;
use crate::targets::{self, RegistryStore};

const PATH_SEPARATOR: char = '.';
const NAME_FIELD: &str = "name";
const SET_RECEIPT_SCHEMA: &str = "stado.registry-set-receipt.v1";

/// The field's place: the value it holds now, so the receipt can say what was
/// replaced, and a mutable borrow of it.
fn leaf<'a>(document: &'a mut Value, path: &str) -> Result<&'a mut Value, CmdError> {
    let mut value = document;
    let mut walked = String::new();
    for segment in path.split(PATH_SEPARATOR).filter(|part| !part.is_empty()) {
        let here = if walked.is_empty() {
            "<root>".to_string()
        } else {
            walked.clone()
        };
        value = step(value, segment, &here)?;
        if !walked.is_empty() {
            walked.push(PATH_SEPARATOR);
        }
        walked.push_str(segment);
    }
    Ok(value)
}

/// One step of the same dotted path `pull --path` reads, mutably. The
/// refusals are the reader's, so a caller that can read a field can write it
/// with the identical path.
fn step<'a>(value: &'a mut Value, segment: &str, walked: &str) -> Result<&'a mut Value, CmdError> {
    match value {
        Value::Object(fields) => {
            if !fields.contains_key(segment) {
                let mut keys: Vec<&str> = fields.keys().map(String::as_str).collect();
                keys.sort_unstable();
                return Err(CmdError::click(format!(
                    "registry has no `{segment}` under `{walked}`; keys there: {}",
                    keys.join(", ")
                )));
            }
            Ok(fields
                .get_mut(segment)
                .unwrap_or_else(|| unreachable!("the key was just found")))
        }
        Value::Array(items) => {
            if let Ok(index) = segment.parse::<usize>() {
                let length = items.len();
                return items.get_mut(index).ok_or_else(|| {
                    CmdError::click(format!(
                        "registry array `{walked}` has {length} element(s), no index {index}"
                    ))
                });
            }
            let names: Vec<String> = items
                .iter()
                .filter_map(|item| item.get(NAME_FIELD).and_then(Value::as_str))
                .map(str::to_string)
                .collect();
            items
                .iter_mut()
                .find(|item| item.get(NAME_FIELD).and_then(Value::as_str) == Some(segment))
                .ok_or_else(|| {
                    CmdError::click(format!(
                        "registry array `{walked}` has no element named `{segment}`; names there: {}",
                        names.join(", ")
                    ))
                })
        }
        other => Err(CmdError::click(format!(
            "registry value at `{walked}` is a {}, which has no `{segment}` inside",
            kind(other)
        ))),
    }
}

/// The value to write: JSON when the argument parses as JSON, the argument
/// itself when it does not, so `--value 8789` is a number, `--value '"8789"'`
/// a string, and `--value /Users/charles/.stado` the path it looks like.
fn parsed(value: &str) -> Value {
    serde_json::from_str(value).unwrap_or_else(|_| Value::String(value.to_string()))
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
    // Read the field first, so a path that does not resolve is refused
    // before anything is serialised, with the reader's own sentence.
    let previous = select(&document, path)?.clone();
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
            &store.location().to_string(),
        );
    }
    *leaf(&mut document, path)? = replacement.clone();
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
