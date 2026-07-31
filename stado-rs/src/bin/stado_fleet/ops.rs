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

/// Remove a target from the document — the rollback half of a verified
/// enroll whose bootstrap failed. Pure.
pub fn remove_target(document: &Value, name: &str) -> Result<Value, String> {
    let mut next = document.clone();
    let targets = next
        .get_mut("targets")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| "registry.targets: must be an array".to_string())?;
    let before = targets.len();
    targets.retain(|target| target.get("name").and_then(Value::as_str) != Some(name));
    if targets.len() == before {
        return Err(format!("target '{name}' not found in registry"));
    }
    Ok(next)
}

/// Probe the machine's real hostname through Stado's own deploy channel —
/// with the target's vault key when one is stored, the OpenSSH default
/// resolution otherwise. Verification BEFORE any registry write: a machine
/// that cannot be reached, or answers with no usable hostname, is never
/// registered.
async fn probe_hostname(
    runner: &stado::deploy::Runner,
    target: &str,
    destination: &str,
) -> Result<String, String> {
    let (argv, materialized) =
        crate::key::channel_argv(runner, target, destination, "hostname").await?;
    let output = runner(stado::deploy::CommandSpec::new(argv)).await?;
    if let Some(path) = materialized {
        let _ = std::fs::remove_file(path);
    }
    if !output.ok() {
        return Err(format!(
            "cannot verify {destination}: {}",
            output.detail()
        ));
    }
    let hostname = stado::targets::normalize_hostname(output.stdout.trim());
    if hostname.is_empty() {
        return Err(format!("{destination} returned an empty hostname"));
    }
    Ok(hostname)
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

/// `stado_fleet enroll NAME --ssh DEST [--kind local] [--fleet FLEET]
/// [--bootstrap]` — verified onboarding as one transaction. The machine is
/// probed through Stado's deploy channel BEFORE anything is written: its
/// real hostname lands in the entry, so the registration is a verified
/// fact, not a declaration. A failed bootstrap rolls the entry back — an
/// unverifiable or uninstallable machine never stays in the registry.
/// Without `--ssh` there is no channel to verify against; the
/// machine-initiated path (`stado_fleet join` there, `approve` here) is
/// the answer for that setup.
pub async fn enroll(
    name: &str,
    ssh: Option<&str>,
    kind: &str,
    fleet_name: Option<&str>,
    bootstrap: bool,
) -> Result<bool, String> {
    let Some(destination) = ssh else {
        return Err(
            "enroll needs --ssh for a verified registration; without a reachable channel use machine-initiated enrollment: stado_fleet join on the machine, then stado_fleet approve here"
                .to_string(),
        );
    };
    let document = fetch_document().await.map_err(|exc| exc.to_string())?;
    crate::enroll::catalog::require_enroll_allowed(&document)?;
    preflight_enroll(&document, name, fleet_name)?;
    let runner = stado::deploy::production_runner();
    let hostname = probe_hostname(&runner, name, destination).await?;
    let next = register_target(&document, name, kind, std::slice::from_ref(&hostname))?;
    let generation = push_document(&next).await.map_err(|exc| exc.to_string())?;
    println!("registered '{name}', verified as '{hostname}' (generation {generation})");
    if let Some(fleet) = fleet_name {
        assign(name, fleet).await?;
    }
    if bootstrap {
        if let Err(exc) = stado::cli::bootstrap::run(Some(name.to_string()), false, false).await {
            let current = fetch_document().await.map_err(|err| err.to_string())?;
            let rolled_back = remove_target(&current, name)?;
            push_document(&rolled_back)
                .await
                .map_err(|err| err.to_string())?;
            return Err(format!(
                "bootstrap failed ({exc}); the registration of '{name}' was rolled back"
            ));
        }
    }
    println!("enrolled '{name}' (kind={kind})");
    Ok(true)
}
