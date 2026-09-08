//! `stado registry host path set` — add or replace one connection path, and
//! the receipt that says whether the registry moved.

use serde_json::{json, Value};

use crate::cli::registry::commands::host::path::registry_host_index;
use crate::cli::registry::write::document::{fetch_versioned_document, push_document_if};
use crate::cli::CmdError;
use crate::targets;

/// One machine-readable answer for every `path set` outcome, including the
/// idempotent one. Desktop clients must not scrape the human sentence to learn
/// whether the registry moved.
fn print_host_path_set_receipt(
    target: &str,
    path: &str,
    destination: &str,
    generation: &str,
    changed: bool,
    json_output: bool,
) -> Result<(), CmdError> {
    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "target": target,
                "path": path,
                "destination": destination,
                "changed": changed,
                "generation": generation,
            }))?
        );
    } else if changed {
        println!("set {target} connection path {path} -> {destination}; generation={generation}");
    } else if path == targets::PRIMARY_SSH_CONNECTION {
        println!("{target}: primary already points to {destination}");
    } else {
        println!("{target}: {path} already points to {destination}");
    }
    Ok(())
}

/// Add or replace one host connection path in the canonical registry.
pub async fn host_path_set(
    host: &str,
    path: &str,
    ssh: &str,
    priority: Option<usize>,
    json_output: bool,
) -> Result<(), CmdError> {
    let path = path.trim();
    let destination = ssh.trim();
    if path.is_empty() {
        return Err(CmdError::click("PATH must not be empty"));
    }
    if destination.is_empty() {
        return Err(CmdError::click("--ssh must not be empty"));
    }
    if priority == Some(0) {
        return Err(CmdError::click("--priority starts at 1"));
    }
    if path == targets::PRIMARY_SSH_CONNECTION && priority.is_some() {
        return Err(CmdError::click(
            "the primary path is always preferred and does not take --priority",
        ));
    }

    let (mut document, expected_generation) = fetch_versioned_document().await?;
    let (index, name) = registry_host_index(&document, host)?;
    let entry = document
        .get_mut("targets")
        .and_then(Value::as_array_mut)
        .and_then(|entries| entries.get_mut(index))
        .and_then(Value::as_object_mut)
        .ok_or_else(|| CmdError::click("registry target must be an object"))?;

    if path == targets::PRIMARY_SSH_CONNECTION {
        if entry.get("ssh").and_then(Value::as_str) == Some(destination) {
            return print_host_path_set_receipt(
                &name,
                path,
                destination,
                &expected_generation,
                false,
                json_output,
            );
        }
        entry.insert("ssh".to_string(), json!(destination));
    } else {
        let paths = entry
            .entry("ssh_fallbacks".to_string())
            .or_insert_with(|| json!([]))
            .as_array_mut()
            .ok_or_else(|| CmdError::click("target.ssh_fallbacks must be an array"))?;
        let existing = paths
            .iter()
            .position(|candidate| candidate.get("name").and_then(Value::as_str) == Some(path));
        let candidate = json!({"name": path, "destination": destination});
        if priority.is_none()
            && existing.is_some_and(|position| paths.get(position) == Some(&candidate))
        {
            return print_host_path_set_receipt(
                &name,
                path,
                destination,
                &expected_generation,
                false,
                json_output,
            );
        }
        let default_position = existing.unwrap_or(paths.len());
        if let Some(position) = existing {
            paths.remove(position);
        }
        let insertion = match priority {
            Some(value) if value > paths.len() + 1 => {
                return Err(CmdError::click(format!(
                    "--priority {value} is outside 1..={}",
                    paths.len() + 1
                )))
            }
            Some(value) => value - 1,
            None => default_position.min(paths.len()),
        };
        paths.insert(insertion, candidate);
    }

    let generation = push_document_if(&document, &expected_generation).await?;
    print_host_path_set_receipt(&name, path, destination, &generation, true, json_output)
}
