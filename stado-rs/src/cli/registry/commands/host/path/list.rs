//! `stado registry host path list` — every declared connection path of one
//! target, in the order the channel tries them.

use serde_json::{json, Value};

use crate::cli::registry::commands::host::path::registry_host_index;
use crate::cli::registry::write::document::fetch_versioned_document;
use crate::cli::CmdError;
use crate::targets;
use crate::targets::ComputeTarget;

/// List the preferred host connection followed by every ordered fallback.
pub async fn host_path_list(host: &str, json_output: bool) -> Result<(), CmdError> {
    let (document, _) = fetch_versioned_document().await?;
    let (index, name) = registry_host_index(&document, host)?;
    let entry = document
        .get("targets")
        .and_then(Value::as_array)
        .and_then(|entries| entries.get(index))
        .cloned()
        .ok_or_else(|| CmdError::click("registry target disappeared"))?;
    let target: ComputeTarget = serde_json::from_value(entry)?;
    let connections = target
        .ssh_connections()
        .enumerate()
        .map(|(order, (path, destination))| {
            json!({
                "name": path,
                "destination": destination,
                "order": order,
                "preferred": order == 0,
            })
        })
        .collect::<Vec<_>>();
    // The vocabulary travels with the listing because this is where a choice
    // is made: an operator or Stado Desktop reading which routes a host has
    // is the same reader that needs to know which networks the product can
    // describe. `declared` marks the ones it does, and a fleet-specific name
    // outside the vocabulary stays legitimate.
    let providers = targets::declared_connection_providers()
        .iter()
        .map(|provider| json!({"name": provider.name, "summary": provider.summary}))
        .collect::<Vec<_>>();
    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "target": name,
                "connections": connections,
                "known_providers": providers,
            }))?
        );
        return Ok(());
    }
    if connections.is_empty() {
        println!("{name}: no SSH connection paths");
    } else {
        for connection in &connections {
            let path = connection["name"].as_str().unwrap_or_default();
            let destination = connection["destination"].as_str().unwrap_or_default();
            let role = if connection["preferred"].as_bool().unwrap_or(false) {
                "preferred"
            } else {
                "fallback"
            };
            let known = if targets::connection_provider_declared(path) {
                "declared"
            } else {
                "fleet-specific"
            };
            println!("{name}\t{path}\t{role}\t{known}\t{destination}");
        }
    }
    let names = targets::declared_connection_providers()
        .iter()
        .map(|provider| provider.name.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    println!("networks this product describes: {names}");
    Ok(())
}
