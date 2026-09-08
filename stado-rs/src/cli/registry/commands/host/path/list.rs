//! `stado registry host path list` — every declared connection path of one
//! target, in the order the channel tries them.

use serde_json::{json, Value};

use crate::cli::registry::commands::host::path::registry_host_index;
use crate::cli::registry::write::document::fetch_versioned_document;
use crate::cli::CmdError;
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
    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "target": name,
                "connections": connections,
            }))?
        );
    } else if connections.is_empty() {
        println!("{name}: no SSH connection paths");
    } else {
        for connection in connections {
            let path = connection["name"].as_str().unwrap_or_default();
            let destination = connection["destination"].as_str().unwrap_or_default();
            let role = if connection["preferred"].as_bool().unwrap_or(false) {
                "preferred"
            } else {
                "fallback"
            };
            println!("{name}\t{path}\t{role}\t{destination}");
        }
    }
    Ok(())
}
