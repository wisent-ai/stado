//! Whether a recipe exists and whether the poller builds it: `remove`, and
//! the `enabled` flag `enable` and `disable` own.

use serde_json::{json, Value};

use crate::cli::builds::{
    builds_array, entry_name, fetch_mutation_document, find_entry, normalized_recipe_json,
    print_json,
};
use crate::cli::CmdError;

pub(in crate::cli::builds) async fn remove(name: &str, json: bool) -> Result<(), CmdError> {
    let (mut document, generation) = fetch_mutation_document().await?;
    let entries = builds_array(&mut document)?;
    let before = entries.len();
    entries.retain(|entry| entry_name(entry) != Some(name));
    if entries.len() == before {
        return Err(CmdError::click(format!(
            "registry declares no build recipe {name:?}"
        )));
    }
    crate::cli::registry::push_document_if(&document, &generation).await?;
    if json {
        return print_json(&json!({ "name": name, "removed": true }));
    }
    println!("{name}: removed");
    Ok(())
}

pub(in crate::cli::builds) async fn set_enabled(
    name: &str,
    enabled: bool,
    json: bool,
) -> Result<(), CmdError> {
    let (mut document, generation) = fetch_mutation_document().await?;
    let entry = find_entry(builds_array(&mut document)?, name)?;
    entry
        .as_object_mut()
        .ok_or_else(|| CmdError::click(format!("build recipe {name:?} must be an object")))?
        .insert("enabled".to_string(), Value::Bool(enabled));
    let updated = normalized_recipe_json(entry);
    crate::cli::registry::push_document_if(&document, &generation).await?;
    if json {
        return print_json(&updated);
    }
    println!("{name}: {}", if enabled { "enabled" } else { "disabled" });
    Ok(())
}
