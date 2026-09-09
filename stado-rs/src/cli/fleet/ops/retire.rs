//! `stado fleet delete`: the pure transform that drops a fleet entry and the
//! command that commits it.

use crate::cli::registry::commit_document;
use serde_json::Value;

use crate::cli::fleet::fleets::{find_fleet, parse_fleets};

/// Remove a fleet entry from the document. A fleet that still has members
/// is refused with their names: dropping the declaration underneath them
/// would produce the dangling `fleet` reference every reader rejects, so
/// the write that would strand them never happens. Pure.
pub fn delete_fleet(document: &Value, name: &str) -> Result<Value, String> {
    let fleets = parse_fleets(document)?;
    let fleet =
        find_fleet(&fleets, name).ok_or_else(|| format!("fleet '{name}' is not declared"))?;
    if !fleet.members.is_empty() {
        return Err(format!(
            "fleet '{name}' still has {} member(s): {}; reassign them first",
            fleet.members.len(),
            fleet.members.join(", ")
        ));
    }
    let mut next = document.clone();
    let section = next
        .get_mut("fleets")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| "registry.fleets: must be an array".to_string())?;
    section.retain(|entry| entry.get("name").and_then(Value::as_str) != Some(name));
    parse_fleets(&next)?;
    Ok(next)
}

/// `stado fleet delete NAME` — retire a declared fleet.
pub async fn delete(name: &str) -> Result<bool, String> {
    // Pure, and the member check has to be re-run against the newer document
    // anyway: a fleet that gained a member since this command started is one
    // whose declaration must not be dropped.
    let generation = commit_document(|document| {
        delete_fleet(document, name).map_err(crate::cli::CmdError::click)
    })
    .await
    .map_err(|exc| exc.to_string())?;
    println!("fleet '{name}' deleted (generation {generation})");
    Ok(true)
}
