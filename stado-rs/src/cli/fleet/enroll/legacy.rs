//! Safe repair of legacy registry entries that declared a machine without a
//! communication channel or any proof of contact.

use crate::monitor::host_health::HostHealthError;
use crate::queue::JobStorage;
use serde_json::{json, Value};

fn target_index(document: &Value, name: &str) -> Result<Option<usize>, String> {
    let targets = document
        .get("targets")
        .and_then(Value::as_array)
        .ok_or_else(|| "registry.targets: must be an array".to_string())?;
    Ok(targets
        .iter()
        .position(|target| target.get("name").and_then(Value::as_str) == Some(name)))
}

/// Refuse takeover unless an existing target is exactly the unverified legacy
/// shape: no channel and no health beacon. Store failures fail closed.
pub async fn allow_takeover(document: &Value, name: &str) -> Result<bool, String> {
    let Some(index) = target_index(document, name)? else {
        return Ok(false);
    };
    let target = &document["targets"][index];
    if target.get("ssh").is_some_and(|value| !value.is_null()) {
        return Err(format!(
            "target '{name}' is already registered with a communication channel"
        ));
    }

    let store = JobStorage::new().await.map_err(|error| error.to_string())?;
    match crate::monitor::host_health::load_host_health(&store, name).await {
        Err(HostHealthError::NoBeacon { .. }) => Ok(true),
        Ok(_) => Err(format!(
            "target '{name}' already has a health beacon and cannot be replaced"
        )),
        Err(error) => Err(format!(
            "cannot prove target '{name}' has no beacon: {error}"
        )),
    }
}

/// Install verified identity and channel into a new or eligible legacy target,
/// preserving capacity and policy fields on the latter.
pub fn register_verified(
    document: &Value,
    name: &str,
    destination: &str,
    kind: &str,
    hostname: &str,
    release_platform: &str,
    takeover: bool,
) -> Result<Value, String> {
    let mut next = document.clone();
    let targets = next
        .get_mut("targets")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| "registry.targets: must be an array".to_string())?;

    if let Some(target) = targets
        .iter_mut()
        .find(|target| target.get("name").and_then(Value::as_str) == Some(name))
    {
        if !takeover {
            return Err(format!("target '{name}' is already registered"));
        }
        let declared_platform = target
            .get("release_platform")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if !declared_platform.is_empty() && declared_platform != release_platform {
            return Err(format!(
                "target '{name}' declares release_platform {declared_platform}, \
                 but enrollment observed {release_platform}"
            ));
        }
        target["ssh"] = Value::String(destination.to_string());
        target["kind"] = Value::String(kind.to_string());
        target["hostnames"] = json!([hostname]);
        target["release_platform"] = Value::String(release_platform.to_string());
        target["notes"] = Value::String(
            "legacy declaration repaired by verified `stado fleet enroll`".to_string(),
        );
        return Ok(next);
    }

    targets.push(json!({
        "name": name,
        "kind": kind,
        "ssh": destination,
        "release_platform": release_platform,
        "hostnames": [hostname],
        "notes": "enrolled by verified `stado fleet enroll`",
    }));
    Ok(next)
}

/// Roll back only the target touched by enrollment, preserving concurrent
/// changes elsewhere in the registry.
pub fn rollback_registration(
    current: &Value,
    original: &Value,
    name: &str,
    takeover: bool,
) -> Result<Value, String> {
    let mut next = current.clone();
    let targets = next
        .get_mut("targets")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| "registry.targets: must be an array".to_string())?;
    if takeover {
        let previous = original
            .get("targets")
            .and_then(Value::as_array)
            .and_then(|items| {
                items
                    .iter()
                    .find(|target| target.get("name").and_then(Value::as_str) == Some(name))
            })
            .cloned()
            .ok_or_else(|| format!("original target '{name}' disappeared"))?;
        let target = targets
            .iter_mut()
            .find(|target| target.get("name").and_then(Value::as_str) == Some(name))
            .ok_or_else(|| format!("target '{name}' disappeared before rollback"))?;
        *target = previous;
    } else {
        targets.retain(|target| target.get("name").and_then(Value::as_str) != Some(name));
    }
    Ok(next)
}
