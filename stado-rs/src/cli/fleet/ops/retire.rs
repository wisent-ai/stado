//! `stado fleet delete`: the pure transform that drops a fleet entry and the
//! command that commits it.

use crate::cli::registry::commit_document;
use crate::cli::CmdError;
use crate::primitives::failure::FailureCode;
use serde_json::Value;

use crate::cli::fleet::fleets::{find_fleet, parse_fleets};

/// Remove a fleet entry from the document. A fleet that still has members
/// is refused with their names: dropping the declaration underneath them
/// would produce the dangling `fleet` reference every reader rejects, so
/// the write that would strand them never happens. Pure.
pub fn delete_fleet(document: &Value, name: &str) -> Result<Value, CmdError> {
    let fleets = parse_fleets(document).map_err(CmdError::declaration)?;
    let fleet = find_fleet(&fleets, name).ok_or_else(|| {
        CmdError::click(format!("fleet '{name}' is not declared")).stating(FailureCode::NotFound)
    })?;
    if !fleet.members.is_empty() {
        return Err(CmdError::refused(format!(
            "fleet '{name}' still has {} member(s): {}; move each with `stado fleet assign TARGET OTHER_FLEET` or take it out with `stado fleet unassign TARGET` first",
            fleet.members.len(),
            fleet.members.join(", ")
        )));
    }
    let mut next = document.clone();
    let section = next
        .get_mut("fleets")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| CmdError::declaration("registry.fleets: must be an array"))?;
    section.retain(|entry| entry.get("name").and_then(Value::as_str) != Some(name));
    parse_fleets(&next).map_err(CmdError::declaration)?;
    Ok(next)
}

/// `stado fleet delete NAME` — retire a declared fleet.
pub async fn delete(name: &str, as_json: bool) -> Result<bool, CmdError> {
    // Pure, and the member check has to be re-run against the newer document
    // anyway: a fleet that gained a member since this command started is one
    // whose declaration must not be dropped.
    let generation = commit_document(|document| delete_fleet(document, name)).await?;
    if as_json {
        let answer = serde_json::json!({ "deleted": name, "generation": generation });
        crate::cli::print_answer(&answer, true)?;
    } else {
        println!("fleet '{name}' deleted (generation {generation})");
    }
    Ok(true)
}
