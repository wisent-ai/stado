//! `stado registry host edit` — change what `host add` declared about a
//! machine, validated before anything is written.

use serde_json::{json, Value};

use crate::cli::registry::commands::host::path::registry_host_index;
use crate::cli::registry::write::document::{fetch_versioned_document, push_document_if};
use crate::cli::CmdError;
use crate::targets;

/// `stado registry host edit HOST [--ssh DEST] [--kind KIND]
/// [--release-platform PLATFORM]` — rewrite the named fields of one target.
///
/// The fields are the ones `host add` writes; any other field is changed
/// through `stado registry set --path targets.<host>.<field>`.
pub async fn host_edit(
    host: &str,
    ssh: Option<&str>,
    kind: Option<&str>,
    release_platform: Option<&str>,
    json_output: bool,
) -> Result<(), CmdError> {
    if ssh.is_none() && kind.is_none() && release_platform.is_none() {
        return Err(CmdError::usage(
            "name at least one of --ssh, --kind or --release-platform to change",
        ));
    }
    if ssh.is_some_and(|value| value.trim().is_empty()) {
        return Err(CmdError::usage("--ssh must not be empty"));
    }
    let location = targets::registry_location();
    let (mut document, expected_generation) = fetch_versioned_document().await?;
    let (index, name) = registry_host_index(&document, host)?;
    let entry = document
        .get_mut("targets")
        .and_then(Value::as_array_mut)
        .and_then(|entries| entries.get_mut(index))
        .and_then(Value::as_object_mut)
        .ok_or_else(|| {
            CmdError::click("registry target must be an object")
                .stating(crate::primitives::failure::FailureCode::Config)
        })?;
    let mut changed = serde_json::Map::new();
    for (field, value) in [
        ("ssh", ssh),
        ("kind", kind),
        ("release_platform", release_platform),
    ] {
        let Some(value) = value else { continue };
        let previous = entry.insert(field.to_string(), json!(value));
        if previous.as_ref().and_then(Value::as_str) != Some(value) {
            changed.insert(field.to_string(), json!({"from": previous, "to": value}));
        }
    }
    let generation = if changed.is_empty() {
        expected_generation
    } else {
        push_document_if(&document, &expected_generation).await?
    };
    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "target": name,
                "changed": changed,
                "generation": generation,
                "registry": location.to_string(),
            }))?
        );
    } else if changed.is_empty() {
        println!("{name}: already declares those values in {location}; nothing written");
    } else {
        let fields: Vec<&str> = changed.keys().map(String::as_str).collect();
        println!(
            "edited {name} ({}) in {location}; generation={generation}",
            fields.join(", ")
        );
    }
    Ok(())
}
