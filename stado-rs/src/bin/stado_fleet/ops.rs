//! Fleet write operations: `create` and `assign`.
//!
//! Every write is a pure document-to-document transform followed by the
//! validated compare-and-swap `push_document` — the exact write path of
//! `stado registry push` — so a malformed fleet change is refused before
//! anything reaches the canonical registry.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use stado::cli::registry::{fetch_document, push_document};
use stado::deploy::{runner_fn, CommandSpec, Runner};
use stado::queue::{capacity, JobStorage};
use stado::targets::{normalize_hostname, ComputeTarget};

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

fn target_has_agent_attestation(target: &Value) -> bool {
    target
        .pointer("/agent_enrollment/status")
        .and_then(Value::as_str)
        == Some("enrolled")
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
            if !target_has_agent_attestation(target) {
                return Err(format!(
                    "target '{target_name}' has no agent enrollment attestation; reconcile it first"
                ));
            }
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

/// Legacy declaration transform retained only for regression tests. It is
/// not compiled into the production binary: every shipped enrollment path
/// must install and attest an agent before registration.
#[cfg(test)]
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

/// Resolve the machine's real hostname through the supplied SSH channel, or
/// locally when repairing the current host. Verification happens before any
/// registry write.
async fn probe_hostname(
    runner: &stado::deploy::Runner,
    target: &str,
    destination: Option<&str>,
) -> Result<String, String> {
    let hostname = if let Some(destination) = destination {
        let (argv, _key) = crate::key::channel_argv(target, destination, "hostname").await?;
        let output = runner(stado::deploy::CommandSpec::new(argv)).await?;
        if !output.ok() {
            return Err(format!("cannot verify {destination}: {}", output.detail()));
        }
        output.stdout
    } else {
        stado::providers::vast::system_hostname()
    };
    let hostname = stado::targets::normalize_hostname(hostname.trim());
    if hostname.is_empty() {
        return Err("machine returned an empty hostname".to_string());
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

/// Complete the legacy cutover only when every local target already carries a
/// valid live-agent receipt. The registry validator rejects the write and
/// names the first unreconciled target; no partial policy change can land.
pub async fn enforce_attestation() -> Result<bool, String> {
    let mut document = fetch_document().await.map_err(|error| error.to_string())?;
    let root = document
        .as_object_mut()
        .ok_or_else(|| "registry must be an object".to_string())?;
    let enrollment = root
        .entry("enrollment".to_string())
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or_else(|| "registry.enrollment: must be an object".to_string())?;
    enrollment.insert("require_agent_attestation".to_string(), Value::Bool(true));
    let generation = push_document(&document)
        .await
        .map_err(|error| error.to_string())?;
    println!("agent attestation enforcement enabled (generation {generation})");
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
    if document
        .get("provisioning_targets")
        .and_then(Value::as_array)
        .is_some_and(|targets| {
            targets
                .iter()
                .any(|target| target.get("name").and_then(Value::as_str) == Some(name))
        })
    {
        return Err(format!("target '{name}' is already being provisioned"));
    }
    if let Some(fleet) = fleet_name {
        let fleets = parse_fleets(document)?;
        find_fleet(&fleets, fleet)
            .ok_or_else(|| format!("fleet '{fleet}' is not declared; create it first"))?;
    }
    Ok(())
}

const AGENT_ATTESTATION_TIMEOUT: Duration = Duration::from_secs(120);
const AGENT_ATTESTATION_POLL: Duration = Duration::from_secs(2);

fn staged_target(document: &Value, name: &str) -> Result<ComputeTarget, String> {
    let target = document
        .get("provisioning_targets")
        .and_then(Value::as_array)
        .and_then(|targets| {
            targets
                .iter()
                .find(|target| target.get("name").and_then(Value::as_str) == Some(name))
        })
        .cloned()
        .ok_or_else(|| format!("provisioning target '{name}' is absent"))?;
    serde_json::from_value(target).map_err(|error| error.to_string())
}

fn same_machine_identity(consumer_id: &str, kind: &str, name: &str, hostname: &str) -> bool {
    let consumer_host = consumer_id
        .strip_prefix(&format!("{kind}-"))
        .unwrap_or(consumer_id);
    let first_label = |value: &str| {
        normalize_hostname(value)
            .split('.')
            .next()
            .unwrap_or_default()
            .to_string()
    };
    let consumer = normalize_hostname(consumer_host);
    consumer == normalize_hostname(name)
        || consumer == normalize_hostname(hostname)
        || first_label(&consumer) == first_label(name)
        || first_label(&consumer) == first_label(hostname)
}

async fn bootstrap_target(target: &ComputeTarget, runner: &Runner) -> Result<(), String> {
    if target.ssh.is_none() {
        let mut echo = |line: &str| println!("{line}");
        return stado::deploy::bootstrap::provision_target(target, false, runner, &mut echo)
            .await
            .map_err(|error| error.to_string());
    }
    let key = Arc::new(
        stado::deploy::ssh_key::materialize(&target.name)
            .await
            .map_err(|error| error.to_string())?,
    );
    let base_runner = Arc::clone(runner);
    let keyed_runner = runner_fn(move |mut spec: CommandSpec| {
        let base_runner = Arc::clone(&base_runner);
        let key = Arc::clone(&key);
        async move {
            if matches!(spec.argv.first().map(String::as_str), Some("ssh" | "scp")) {
                spec.argv = stado::deploy::ssh_key::add_identity(spec.argv, &key)
                    .map_err(|error| error.to_string())?;
            }
            base_runner(spec).await
        }
    });
    let mut echo = |line: &str| println!("{line}");
    stado::deploy::bootstrap::provision_target(target, false, &keyed_runner, &mut echo)
        .await
        .map_err(|error| error.to_string())
}

async fn wait_for_agent_attestation(
    name: &str,
    hostname: &str,
    kind: &str,
    baseline: &BTreeMap<String, Value>,
) -> Result<Value, String> {
    let store = JobStorage::new().await.map_err(|error| error.to_string())?;
    let deadline = tokio::time::Instant::now() + AGENT_ATTESTATION_TIMEOUT;
    loop {
        let current = capacity::read_consumer_capacity(&store)
            .await
            .map_err(|error| error.to_string())?;
        for (consumer_id, report) in current {
            if !same_machine_identity(&consumer_id, kind, name, hostname) {
                continue;
            }
            let previous_publication = baseline
                .get(&consumer_id)
                .and_then(|value| value.get("published_at"))
                .and_then(Value::as_str);
            let published_at = report
                .get("published_at")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    format!("agent '{consumer_id}' published capacity without published_at")
                })?;
            if previous_publication == Some(published_at) {
                continue;
            }
            let version = report
                .get("stado_version")
                .and_then(Value::as_str)
                .filter(|version| !version.is_empty())
                .ok_or_else(|| {
                    format!("agent '{consumer_id}' published capacity without stado_version")
                })?;
            return Ok(json!({
                "status": "enrolled",
                "attested_at": chrono::Utc::now().to_rfc3339(),
                "consumer_id": consumer_id,
                "hostname": hostname,
                "kind": kind,
                "stado_version": version,
                "capacity_published_at": published_at,
            }));
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(format!(
                "agent for '{name}' did not publish fresh capacity within {} seconds",
                AGENT_ATTESTATION_TIMEOUT.as_secs()
            ));
        }
        tokio::time::sleep(AGENT_ATTESTATION_POLL).await;
    }
}

async fn rollback_staging(
    original: &Value,
    name: &str,
    takeover: bool,
    cause: &str,
) -> Result<String, String> {
    let current = fetch_document().await.map_err(|error| error.to_string())?;
    let rolled_back =
        crate::enroll::legacy::rollback_registration(&current, original, name, takeover)?;
    push_document(&rolled_back)
        .await
        .map_err(|error| error.to_string())?;
    Ok(format!(
        "{cause}; provisioning target '{name}' was removed from the registry"
    ))
}

/// Verified onboarding is one transaction: probe SSH, stage a non-routable
/// provisioning target, install the agent, require a fresh capacity
/// attestation, and only then promote the machine into `targets` and a fleet.
pub async fn enroll(
    name: &str,
    destination: Option<&str>,
    kind: &str,
    fleet_name: Option<&str>,
    expected_hostname: Option<&str>,
) -> Result<bool, String> {
    let document = fetch_document().await.map_err(|error| error.to_string())?;
    crate::enroll::catalog::require_enroll_allowed(&document)?;
    let takeover = crate::enroll::legacy::allow_takeover(&document, name)?;
    if takeover {
        if let Some(fleet) = fleet_name {
            let fleets = parse_fleets(&document)?;
            find_fleet(&fleets, fleet)
                .ok_or_else(|| format!("fleet '{fleet}' is not declared; create it first"))?;
        }
    } else {
        preflight_enroll(&document, name, fleet_name)?;
    }

    let runner = stado::deploy::production_runner();
    let hostname = probe_hostname(&runner, name, destination).await?;
    if expected_hostname
        .is_some_and(|expected| normalize_hostname(expected) != normalize_hostname(&hostname))
    {
        return Err(format!(
            "join request identifies '{}', but SSH reached '{hostname}'",
            expected_hostname.unwrap_or_default()
        ));
    }
    let staged = crate::enroll::legacy::stage_verified(
        &document,
        name,
        destination,
        kind,
        &hostname,
        takeover,
    )?;
    let staged_generation = push_document(&staged)
        .await
        .map_err(|error| error.to_string())?;
    println!(
        "provisioning '{name}' as '{hostname}' (generation {staged_generation}); not registered yet"
    );

    let store = JobStorage::new().await.map_err(|error| error.to_string())?;
    let baseline = capacity::read_consumer_capacity(&store)
        .await
        .map_err(|error| error.to_string())?;
    let target = staged_target(&staged, name)?;
    if let Err(error) = bootstrap_target(&target, &runner).await {
        return Err(rollback_staging(
            &document,
            name,
            takeover,
            &format!("agent bootstrap failed: {error}"),
        )
        .await?);
    }
    let attestation = match wait_for_agent_attestation(name, &hostname, kind, &baseline).await {
        Ok(attestation) => attestation,
        Err(error) => {
            return Err(rollback_staging(&document, name, takeover, &error).await?);
        }
    };

    let current = fetch_document().await.map_err(|error| error.to_string())?;
    let finalized =
        crate::enroll::legacy::finalize_registration(&current, name, fleet_name, attestation)?;
    parse_fleets(&finalized)?;
    let generation = match push_document(&finalized).await {
        Ok(generation) => generation,
        Err(error) => {
            return Err(rollback_staging(&document, name, takeover, &error.to_string()).await?);
        }
    };
    println!("enrolled '{name}' (kind={kind}, hostname={hostname}, generation {generation})");
    Ok(true)
}

/// Repair an existing unverified entry through the same enrollment
/// transaction. An active attested target is refused rather than rewritten.
pub async fn reconcile(name: &str) -> Result<bool, String> {
    let document = fetch_document().await.map_err(|error| error.to_string())?;
    let target = document
        .get("targets")
        .and_then(Value::as_array)
        .and_then(|targets| {
            targets
                .iter()
                .find(|target| target.get("name").and_then(Value::as_str) == Some(name))
        })
        .ok_or_else(|| format!("target '{name}' is not registered"))?;
    let destination = target
        .get("ssh")
        .and_then(Value::as_str)
        .filter(|ssh| !ssh.is_empty())
        .map(str::to_string);
    if destination.is_none() {
        let local_hostname = normalize_hostname(&stado::providers::vast::system_hostname());
        let matches_local = target
            .get("hostnames")
            .and_then(Value::as_array)
            .is_some_and(|hostnames| {
                hostnames.iter().any(|hostname| {
                    hostname
                        .as_str()
                        .is_some_and(|hostname| normalize_hostname(hostname) == local_hostname)
                })
            });
        if !matches_local {
            return Err(format!(
                "target '{name}' has no SSH channel and is not this machine"
            ));
        }
    }
    let kind = target
        .get("kind")
        .and_then(Value::as_str)
        .unwrap_or("local")
        .to_string();
    let fleet = target
        .get("fleet")
        .and_then(Value::as_str)
        .map(str::to_string);
    enroll(name, destination.as_deref(), &kind, fleet.as_deref(), None).await
}
