//! `stado fleet create`: the pure transform that appends a fleet entry and
//! the command that commits it.

use crate::cli::registry::commit_document;
use serde_json::{json, Value};

use crate::cli::fleet::fleets::{find_fleet, parse_fleets};

/// Append a fleet entry to the document. Duplicate names are refused up
/// front; the result is re-parsed through the same [`parse_fleets`] the
/// readers use, so an invalid name fails here, not in production. Pure.
pub fn create_fleet(document: &Value, name: &str, notes: &str) -> Result<Value, String> {
    let fleets = parse_fleets(document)?;
    if find_fleet(&fleets, name).is_some() {
        return Err(format!("fleet '{name}' already exists"));
    }
    let mut next = document.clone();
    let root = next
        .as_object_mut()
        .ok_or_else(|| "registry must be an object".to_string())?;
    let section = root
        .entry("fleets".to_string())
        .or_insert_with(|| json!([]));
    let entries = section
        .as_array_mut()
        .ok_or_else(|| "registry.fleets: must be an array".to_string())?;
    entries.push(json!({ "name": name, "notes": notes }));
    parse_fleets(&next)?;
    Ok(next)
}

/// `stado fleet create NAME` — declare a fleet in the canonical registry.
pub async fn create(name: &str, notes: &str) -> Result<bool, String> {
    // Pure: the fleet entry is a function of the document it is appended to,
    // so a lost race is answered by appending it to the newer document.
    let generation = commit_document(|document| {
        create_fleet(document, name, notes).map_err(crate::cli::CmdError::click)
    })
    .await
    .map_err(|exc| exc.to_string())?;
    println!("fleet '{name}' created (generation {generation})");
    Ok(true)
}
