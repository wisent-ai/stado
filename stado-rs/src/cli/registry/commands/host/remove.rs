//! `stado registry host remove` — retire a machine from the canonical
//! registry, refused while anything else in the document still names it.

use serde_json::Value;

use crate::cli::registry::commands::host::path::registry_host_index;
use crate::cli::registry::write::document::{fetch_versioned_document, push_document_if};
use crate::cli::CmdError;
use crate::targets;

/// Every place outside the host's own block whose value is the host's name,
/// as the dotted path `registry pull --path` reads.
fn references(value: &Value, name: &str, path: &str, found: &mut Vec<String>) {
    match value {
        Value::String(text) if targets::normalize_hostname(text) == name => {
            found.push(path.to_string());
        }
        Value::Array(items) => {
            for (index, item) in items.iter().enumerate() {
                references(item, name, &format!("{path}[{index}]"), found);
            }
        }
        Value::Object(fields) => {
            for (key, item) in fields {
                let child = if path.is_empty() {
                    key.clone()
                } else {
                    format!("{path}.{key}")
                };
                if targets::normalize_hostname(key) == name {
                    found.push(child.clone());
                }
                references(item, name, &child, found);
            }
        }
        _ => {}
    }
}

/// `stado registry host remove HOST` — delete one target from the registry.
///
/// A host that a service, database or other section still names is refused
/// with those paths listed, because removing it would leave the registry
/// pointing at a machine it no longer declares.
pub async fn host_remove(host: &str, json_output: bool) -> Result<(), CmdError> {
    let location = targets::registry_location();
    let (mut document, expected_generation) = fetch_versioned_document().await?;
    let (index, name) = registry_host_index(&document, host)?;
    let entries = document
        .get_mut("targets")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| CmdError::click("registry.targets: must be an array"))?;
    let removed = entries.remove(index);
    let mut found = Vec::new();
    references(&document, &name, "", &mut found);
    if !found.is_empty() {
        return Err(CmdError::refused(format!(
            "{name} is still named at {} in {location}; move or remove those entries first \
             (`stado registry set --path <path> --value <other host>`), then remove {name}",
            found.join(", ")
        )));
    }
    let generation = push_document_if(&document, &expected_generation).await?;
    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "target": name,
                "removed": removed,
                "generation": generation,
                "registry": location.to_string(),
            }))?
        );
    } else {
        println!("removed {name} from {location}; generation={generation}");
    }
    Ok(())
}
