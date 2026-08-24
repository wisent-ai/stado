//! Fleet write operations: `create` and `assign`.
//!
//! Every write is a pure document-to-document transform followed by the
//! validated compare-and-swap `push_document` — the exact write path of
//! `stado registry push` — so a malformed fleet change is refused before
//! anything reaches the canonical registry.

use crate::cli::registry::{fetch_document, push_document};
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

/// `stado fleet create NAME` — declare a fleet in the canonical registry.
pub async fn create(name: &str, notes: &str) -> Result<bool, String> {
    let document = fetch_document().await.map_err(|exc| exc.to_string())?;
    let next = create_fleet(&document, name, notes)?;
    let generation = push_document(&next).await.map_err(|exc| exc.to_string())?;
    println!("fleet '{name}' created (generation {generation})");
    Ok(true)
}

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
    let document = fetch_document().await.map_err(|exc| exc.to_string())?;
    let next = delete_fleet(&document, name)?;
    let generation = push_document(&next).await.map_err(|exc| exc.to_string())?;
    println!("fleet '{name}' deleted (generation {generation})");
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
    release_platform: &str,
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
        "release_platform": release_platform,
        "ssh": Value::Null,
        "hostnames": hostnames,
        "notes": "enrolled by `stado fleet enroll` (self-install path)",
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

/// Probe one fixed identity command through Stado's existing deploy channel.
async fn probe_identity_field(
    runner: &crate::deploy::Runner,
    target: &str,
    destination: &str,
    command: &str,
) -> Result<String, String> {
    let (argv, _key) = crate::cli::fleet::key::channel_argv(target, destination, command).await?;
    let output = runner(crate::deploy::CommandSpec::new(argv)).await?;
    if !output.ok() {
        return Err(format!("cannot verify {destination}: {}", output.detail()));
    }
    let value = output.stdout.trim();
    if value.is_empty() || value.lines().count() != 1 {
        return Err(format!("{destination} returned an invalid {command} value"));
    }
    Ok(value.to_string())
}

/// Verify hostname and immutable-release platform before any registry write.
async fn probe_identity(
    runner: &crate::deploy::Runner,
    target: &str,
    destination: &str,
) -> Result<(String, &'static str), String> {
    let raw_hostname = probe_identity_field(runner, target, destination, "hostname").await?;
    let hostname = crate::targets::normalize_hostname(&raw_hostname);
    if hostname.is_empty() {
        return Err(format!("{destination} returned an empty hostname"));
    }
    let os = probe_identity_field(runner, target, destination, "uname -s").await?;
    let arch = probe_identity_field(runner, target, destination, "uname -m").await?;
    let platform = crate::cli::fleet::enroll::release_platform(&os, &arch)?;
    Ok((hostname, platform))
}

/// `stado fleet assign TARGET FLEET` — add a registered machine to a fleet.
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

/// `stado fleet enroll NAME --ssh DEST [--install-key] [--kind local]
/// [--fleet FLEET] [--bootstrap]` — verified onboarding as one transaction.
/// The machine is probed through Stado's deploy channel BEFORE anything is
/// written: its real hostname lands in the entry, so the registration is a
/// verified fact, not a declaration. A failed bootstrap rolls the entry back
/// — an unverifiable or uninstallable machine never stays in the registry.
/// Without `--ssh` there is no channel to verify against; the
/// machine-initiated path (`stado fleet join` there, `approve` here) is
/// the answer for that setup.
///
/// `install_key` is the `adopt` method: the deploy channel needs the fleet's
/// key to already be in the machine's authorized_keys, and on a machine nobody
/// has adopted yet it is not, which is why enrolling used to start with an
/// operator pasting a public key by hand. With the flag, Stado puts it there
/// itself over a session the operator can already open by other means, and
/// then the run continues down exactly the path below — probe, write, optional
/// bootstrap, rollback — with nothing else changed. The install happens before
/// the probe because the probe is the first thing that needs the key, and
/// before any registry write, so a machine that cannot be adopted leaves no
/// entry behind.
pub async fn enroll(
    name: &str,
    ssh: Option<&str>,
    kind: &str,
    fleet_name: Option<&str>,
    bootstrap: bool,
    install_key: bool,
) -> Result<bool, String> {
    let Some(destination) = ssh else {
        return Err(
            "enroll needs --ssh for a verified registration; without a reachable channel use machine-initiated enrollment: stado fleet join on the machine, then stado fleet approve here"
                .to_string(),
        );
    };
    let document = fetch_document().await.map_err(|exc| exc.to_string())?;
    crate::cli::fleet::enroll::catalog::require_enroll_allowed(&document)?;
    if install_key {
        crate::cli::fleet::enroll::catalog::require_adopt_allowed(&document)?;
    }
    let takeover = crate::cli::fleet::enroll::legacy::allow_takeover(&document, name).await?;
    if takeover {
        if let Some(fleet) = fleet_name {
            let fleets = parse_fleets(&document)?;
            find_fleet(&fleets, fleet)
                .ok_or_else(|| format!("fleet '{fleet}' is not declared; create it first"))?;
        }
    } else {
        preflight_enroll(&document, name, fleet_name)?;
    }
    let runner = crate::deploy::production_runner();
    if install_key {
        crate::cli::fleet::key::install_first_contact(&runner, name, destination).await?;
    }
    let (hostname, release_platform) = probe_identity(&runner, name, destination).await?;
    let mut next = crate::cli::fleet::enroll::legacy::register_verified(
        &document,
        name,
        destination,
        kind,
        &hostname,
        release_platform,
        takeover,
    )?;
    if let Some(fleet) = fleet_name {
        next = assign_target(&next, name, fleet)?;
    }
    let generation = push_document(&next).await.map_err(|exc| exc.to_string())?;
    println!("registered '{name}', verified as '{hostname}' (generation {generation})");
    if bootstrap {
        if let Err(exc) = crate::cli::bootstrap::run(Some(name.to_string()), false, false).await {
            let current = fetch_document().await.map_err(|err| err.to_string())?;
            let rolled_back = if takeover {
                crate::cli::fleet::enroll::legacy::rollback_registration(
                    &current, &document, name, true,
                )?
            } else {
                remove_target(&current, name)?
            };
            push_document(&rolled_back)
                .await
                .map_err(|err| err.to_string())?;
            return Err(format!(
                "bootstrap failed ({exc}); the registration of '{name}' was rolled back"
            ));
        }
    }
    // An offline invite is closed by exactly this: the operator got the address
    // from the machine's owner and registered the name. The registration
    // already stands, so a store that cannot be reached now is a warning, not a
    // reason to fail a run that wrote the registry.
    match crate::cli::fleet::invite::close_offline_for_target(name).await {
        Ok(Some(invite_id)) => println!("offline invite {invite_id} is spent"),
        Ok(None) => {}
        Err(exc) => eprintln!(
            "registration stands, but the offline invite for '{name}' could not be closed: {exc}"
        ),
    }
    println!("enrolled '{name}' (kind={kind})");
    Ok(true)
}
