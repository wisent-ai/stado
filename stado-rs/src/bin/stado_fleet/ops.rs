//! Fleet write operations: `create` and `assign`.
//!
//! Every write is a pure document-to-document transform followed by the
//! validated compare-and-swap `push_document` — the exact write path of
//! `stado registry push` — so a malformed fleet change is refused before
//! anything reaches the canonical registry.

use serde_json::{json, Value};
use stado::cli::registry::{fetch_document, push_document};

use crate::fleet::{find_fleet, parse_fleets};

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

/// `stado_fleet create NAME` — declare a fleet in the canonical registry.
pub async fn create(name: &str, notes: &str) -> Result<bool, String> {
    let document = fetch_document().await.map_err(|exc| exc.to_string())?;
    let next = create_fleet(&document, name, notes)?;
    let generation = push_document(&next).await.map_err(|exc| exc.to_string())?;
    println!("fleet '{name}' created (generation {generation})");
    Ok(true)
}

/// Register a target without an ssh destination (`ssh: null`) — the
/// self-install path: the machine later runs `stado bootstrap --local
/// --target NAME` on itself. `hostnames` carries the machine's real DNS
/// names: the agent resolves itself by hostname (`lookup_self`), so an
/// entry without them only ever matches a machine whose hostname equals
/// the target name. Duplicate names are refused; the result is validated
/// by the registry-v2 contract inside `push_document`. Pure.
pub fn register_target(
    document: &Value,
    name: &str,
    kind: &str,
    hostnames: &[String],
) -> Result<Value, String> {
    let mut next = document.clone();
    let targets = next
        .get_mut("targets")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| "registry.targets: must be an array".to_string())?;
    if targets
        .iter()
        .any(|target| target.get("name").and_then(Value::as_str) == Some(name))
    {
        return Err(format!("target '{name}' is already registered"));
    }
    targets.push(json!({
        "name": name,
        "kind": kind,
        "ssh": Value::Null,
        "hostnames": hostnames,
        "notes": "enrolled by `stado_fleet enroll` (self-install path)",
    }));
    Ok(next)
}

/// `stado_fleet assign TARGET FLEET` — add a registered machine to a fleet.
pub async fn assign(target: &str, fleet_name: &str) -> Result<bool, String> {
    let document = fetch_document().await.map_err(|exc| exc.to_string())?;
    let next = assign_target(&document, target, fleet_name)?;
    let generation = push_document(&next).await.map_err(|exc| exc.to_string())?;
    println!("target '{target}' assigned to fleet '{fleet_name}' (generation {generation})");
    Ok(true)
}

/// Enroll preflight, run BEFORE any write: the machine must not already be
/// registered, and the requested fleet must be declared — otherwise the
/// command would register a target and only then fail the fleet step.
/// Pure.
pub fn preflight_enroll(
    document: &Value,
    name: &str,
    fleet_name: Option<&str>,
) -> Result<(), String> {
    let targets = document
        .get("targets")
        .and_then(Value::as_array)
        .ok_or_else(|| "registry.targets: must be an array".to_string())?;
    if targets
        .iter()
        .any(|target| target.get("name").and_then(Value::as_str) == Some(name))
    {
        return Err(format!("target '{name}' is already registered"));
    }
    if let Some(fleet) = fleet_name {
        let fleets = parse_fleets(document)?;
        find_fleet(&fleets, fleet)
            .ok_or_else(|| format!("fleet '{fleet}' is not declared; create it first"))?;
    }
    Ok(())
}

/// `stado_fleet enroll NAME [--ssh DEST] [--kind local] [--fleet FLEET]
/// [--bootstrap]` — one-command onboarding. With `--ssh`, the agent is
/// installable from here through `stado bootstrap`. Without it, the target
/// registers with `ssh: null` on the self-install path: run
/// `stado bootstrap --local --target NAME` on the machine itself.
/// Registration goes through the same validated CAS write either way.
pub async fn enroll(
    name: &str,
    ssh: Option<&str>,
    kind: &str,
    fleet_name: Option<&str>,
    bootstrap: bool,
    hostname: Option<&str>,
) -> Result<bool, String> {
    let document = fetch_document().await.map_err(|exc| exc.to_string())?;
    preflight_enroll(&document, name, fleet_name)?;
    if bootstrap && ssh.is_none() {
        return Err(
            "--bootstrap needs --ssh; on an ssh-less target run stado bootstrap --local on the machine"
                .to_string(),
        );
    }
    match ssh {
        Some(destination) => {
            stado::cli::registry::host_add(name, destination, kind)
                .await
                .map_err(|exc| exc.to_string())?;
        }
        None => {
            let hostnames: Vec<String> = hostname
                .map(|value| vec![value.to_string()])
                .unwrap_or_default();
            if hostnames.is_empty() {
                println!(
                    "note: no --hostname given; the agent will find this entry only if the machine's hostname is '{name}'"
                );
            }
            let next = register_target(&document, name, kind, &hostnames)?;
            let generation = push_document(&next)
                .await
                .map_err(|exc| exc.to_string())?;
            println!("registered '{name}' (kind={kind}, ssh=null) generation={generation}");
            println!("self-install on the machine: stado bootstrap --local --target '{name}'");
        }
    }
    if let Some(fleet) = fleet_name {
        assign(name, fleet).await?;
    }
    if bootstrap {
        stado::cli::bootstrap::run(Some(name.to_string()), false, false)
            .await
            .map_err(|exc| exc.to_string())?;
    }
    println!("enrolled '{name}' (kind={kind})");
    Ok(true)
}
