//! `stado fleet enroll`: the target-entry transforms, the identity probe run
//! through Stado's deploy channel, the preflight and the command that ties
//! them into one conditional write with a rollback.

use crate::cli::registry::{fetch_versioned_document, push_document_if};
use serde_json::{json, Value};

use crate::cli::fleet::fleets::{find_fleet, parse_fleets};

use super::assignment::assign_target;

/// Register a target without an ssh destination (`ssh: null`) — the
/// self-install path: the machine later runs `stado bootstrap --local
/// --target NAME` on itself. `hostnames` carries the machine's real DNS
/// names: the agent resolves itself by hostname (`lookup_self`), so an
/// entry without them only ever matches a machine whose hostname equals
/// the target name. Duplicate names are refused; the result is validated
/// by the registry-v2 contract inside `push_document_if`. Pure.
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
    // The generation this whole run is conditional on: every check below, and
    // the key install and identity probe that follow them, were decided
    // against THIS document. If it has moved by the time the entry is
    // written, the decisions no longer hold and the operator has to see that.
    let (document, expected_generation) = fetch_versioned_document()
        .await
        .map_err(|exc| exc.to_string())?;
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
    let generation = push_document_if(&next, &expected_generation)
        .await
        .map_err(|exc| exc.to_string())?;
    println!("registered '{name}', verified as '{hostname}' (generation {generation})");
    if bootstrap {
        if let Err(exc) = crate::cli::bootstrap::run(Some(name.to_string()), false, false).await {
            // The rollback's own expected generation: the re-read here is what
            // the removal is computed from, so it is also what the removal is
            // conditional on. A writer that lands in between leaves the entry
            // in place, said out loud, rather than having its edit erased by a
            // rollback that never saw it.
            let (current, current_generation) = fetch_versioned_document()
                .await
                .map_err(|err| err.to_string())?;
            let rolled_back = if takeover {
                crate::cli::fleet::enroll::legacy::rollback_registration(
                    &current, &document, name, true,
                )?
            } else {
                remove_target(&current, name)?
            };
            push_document_if(&rolled_back, &current_generation)
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
