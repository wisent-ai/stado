//! `stado registry host path remove` — drop one declared connection path,
//! and the receipt that tells an idempotent absence from a registry write.

use serde_json::{json, Value};

use crate::cli::registry::commands::host::path::registry_host_index;
use crate::cli::registry::write::document::{fetch_versioned_document, push_document_if};
use crate::cli::CmdError;
use crate::targets;

/// The remove receipt distinguishes an idempotent absence from a registry
/// write while preserving the existing terminal sentence.
fn print_host_path_remove_receipt(
    target: &str,
    path: &str,
    generation: &str,
    removed: bool,
    json_output: bool,
) -> Result<(), CmdError> {
    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "target": target,
                "path": path,
                "removed": removed,
                "generation": generation,
            }))?
        );
    } else if removed {
        println!("removed {target} connection path {path}; generation={generation}");
    } else {
        println!("{target}: connection path {path} is already absent");
    }
    Ok(())
}

/// Remove one fallback path; the preferred path is replaced through `set`.
pub async fn host_path_remove(host: &str, path: &str, json_output: bool) -> Result<(), CmdError> {
    let path = path.trim();
    if path == targets::PRIMARY_SSH_CONNECTION {
        return Err(CmdError::click(
            "the primary path cannot be removed; replace it with `registry host path set`",
        ));
    }
    if path.is_empty() {
        return Err(CmdError::click("PATH must not be empty"));
    }

    let (mut document, expected_generation) = fetch_versioned_document().await?;
    let (index, name) = registry_host_index(&document, host)?;
    let entry = document
        .get_mut("targets")
        .and_then(Value::as_array_mut)
        .and_then(|entries| entries.get_mut(index))
        .and_then(Value::as_object_mut)
        .ok_or_else(|| CmdError::click("registry target must be an object"))?;
    let Some(paths) = entry.get_mut("ssh_fallbacks").and_then(Value::as_array_mut) else {
        return print_host_path_remove_receipt(
            &name,
            path,
            &expected_generation,
            false,
            json_output,
        );
    };
    let Some(position) = paths
        .iter()
        .position(|candidate| candidate.get("name").and_then(Value::as_str) == Some(path))
    else {
        return print_host_path_remove_receipt(
            &name,
            path,
            &expected_generation,
            false,
            json_output,
        );
    };
    paths.remove(position);
    if paths.is_empty() {
        entry.remove("ssh_fallbacks");
    }

    let generation = push_document_if(&document, &expected_generation).await?;
    print_host_path_remove_receipt(&name, path, &generation, true, json_output)
}
