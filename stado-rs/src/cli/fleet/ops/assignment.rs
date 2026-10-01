//! `stado fleet assign`: the pure transform that points a target at a fleet
//! and the command that commits it.

use crate::cli::registry::commit_document;
use serde_json::{json, Value};

use crate::cli::fleet::fleets::{find_fleet, parse_fleets};

/// Point one target's `fleet` field at a declared fleet. Moving a target
/// between fleets is just another assignment; pointing at an undeclared
/// fleet or an unknown target is refused. Pure.
pub fn assign_target(
    document: &Value,
    target_name: &str,
    fleet_name: &str,
) -> Result<Value, String> {
    let fleets = parse_fleets(document)?;
    find_fleet(&fleets, fleet_name)
        .ok_or_else(|| format!("fleet '{fleet_name}' is not declared; create it first"))?;
    let mut next = document.clone();
    let targets = next
        .get_mut("targets")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| "registry.targets: must be an array".to_string())?;
    let mut found = false;
    for target in targets.iter_mut() {
        if target.get("name").and_then(Value::as_str) == Some(target_name) {
            target["fleet"] = Value::String(fleet_name.to_string());
            found = true;
        }
    }
    if !found {
        return Err(format!("target '{target_name}' not found in registry"));
    }
    parse_fleets(&next)?;
    Ok(next)
}

/// `stado fleet assign TARGET FLEET` — add a registered machine to a fleet.
pub async fn assign(target: &str, fleet_name: &str, as_json: bool) -> Result<bool, String> {
    // Pure: the assignment is one field on one target, and re-applying it to
    // a newer document is exactly the intent.
    let generation = commit_document(|document| {
        assign_target(document, target, fleet_name).map_err(crate::cli::CmdError::click)
    })
    .await
    .map_err(|exc| exc.to_string())?;
    if as_json {
        let answer = json!({ "target": target, "fleet": fleet_name, "generation": generation });
        crate::cli::print_answer(&answer, true).map_err(|exc| exc.to_string())?;
    } else {
        println!("target '{target}' assigned to fleet '{fleet_name}' (generation {generation})");
    }
    Ok(true)
}

/// Clear one target's `fleet` field, so the machine belongs to no fleet.
/// An unknown target is refused; a target in no fleet is left as it is.
/// Returns the fleet it left, or `None` when it was in none. Pure.
pub fn unassign_target(
    document: &Value,
    target_name: &str,
) -> Result<(Value, Option<String>), String> {
    let mut next = document.clone();
    let targets = next
        .get_mut("targets")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| "registry.targets: must be an array".to_string())?;
    let target = targets
        .iter_mut()
        .find(|target| target.get("name").and_then(Value::as_str) == Some(target_name))
        .and_then(Value::as_object_mut)
        .ok_or_else(|| format!("target '{target_name}' not found in registry"))?;
    let left = target
        .remove("fleet")
        .and_then(|fleet| fleet.as_str().map(str::to_string));
    parse_fleets(&next)?;
    Ok((next, left))
}

/// `stado fleet unassign TARGET` — take a registered machine out of its fleet.
pub async fn unassign(target: &str, as_json: bool) -> Result<bool, String> {
    let left = std::sync::Mutex::new(None);
    let generation = commit_document(|document| {
        let (next, fleet) =
            unassign_target(document, target).map_err(crate::cli::CmdError::click)?;
        *left.lock().expect("unassign result lock") = fleet;
        Ok(next)
    })
    .await
    .map_err(|exc| exc.to_string())?;
    let left = left.into_inner().expect("unassign result lock");
    if as_json {
        let answer = json!({ "target": target, "left": left, "generation": generation });
        crate::cli::print_answer(&answer, true).map_err(|exc| exc.to_string())?;
        return Ok(true);
    }
    match left {
        Some(fleet) => {
            println!("target '{target}' removed from fleet '{fleet}' (generation {generation})")
        }
        None => println!("target '{target}' was in no fleet; nothing changed"),
    }
    Ok(true)
}
