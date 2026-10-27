//! `stado registry set` — change one field of the canonical registry under
//! the generation it was read at.
//!
//! A field path survives document reordering, unlike a line-number edit.
//! Updating the authority directly also avoids leaving a validated change
//! only in a scratch copy.
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
/// The directory whose readers count on its generation.
const DIRECTORY_KEY: &str = "service_directory";

/// The value to write: JSON when the argument parses as JSON, the argument
/// itself when it does not, so `--value 8789` is a number, `--value '"8789"'`
/// a string, and `--value /Users/<user>/.stado` the path it looks like.
fn parsed(value: &str) -> Value {
    match serde_json::from_str(value) {
        Ok(json) => json,
        // Not JSON: the argument is the text it spells, as documented above.
        Err(_not_json) => Value::String(value.to_string()),
    }
}

/// `stado registry set --path P --value V [--json]`.
pub async fn set(path: &str, value: &str, json_output: bool) -> Result<(), CmdError> {
    if path.trim().is_empty() {
        return Err(CmdError::usage(
            "--path names the field to change, as `registry pull --path` reads it",
        ));
    }
    let store = RegistryStore::open().await?;
    let blob = store
        .read_versioned()
        .await?
        .ok_or_else(|| crate::cli::registry::registry_absent(store.location()))?;
    let mut document: Value = serde_json::from_str(&blob.content)?;
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
    *leaf(&mut document, path)? = replacement;
    targets::strip_retired_resource_declarations(&mut document);
    let replacement = select(&document, path).map_err(|_| {
        CmdError::usage(format!(
            "{path} is a retired declaration and cannot be set; the registry was not changed"
        ))
    })?;
    if &previous == replacement {
        return report(
            json_output,
            "unchanged",
            path,
            &previous,
            replacement,
            &blob.version,
            None,
            store.location(),
        );
    }
    // A service directory that changed and kept its generation is a document
    // every resolver believes it has already read: the validator refuses it,
    // and rightly. The number belongs to the change, so it moves with it here
    // rather than in a second command an operator has to remember.
    // The shared advance every directory writer uses: a directory that is not
    // an object, or a generation that is missing or not a whole number, is
    // refused with the registry unchanged instead of restarting at one.
    if path.starts_with(DIRECTORY_KEY) {
        crate::service_resolution::advance_generation(&mut document)
            .map_err(|error| CmdError::usage(format!("{error}; the registry was not changed")))?;
    }
    let replacement = select(&document, path)?;
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
            replacement,
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

/// A value as the receipt line shows it: text bare, anything else as its JSON.
fn compact(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}
