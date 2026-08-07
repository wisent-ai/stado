//! Safe repair of legacy registry entries that declared a machine without a
//! communication channel or any proof of contact.

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

/// An existing target may be repaired only when it has not already completed
/// agent enrollment. Reconciliation uses its declared SSH channel and requires
/// a new agent attestation before restoring it to the routable target set.
pub fn allow_takeover(document: &Value, name: &str) -> Result<bool, String> {
    let Some(index) = target_index(document, name)? else {
        return Ok(false);
    };
    let target = &document["targets"][index];
    if target
        .pointer("/agent_enrollment/status")
        .and_then(Value::as_str)
        == Some("enrolled")
    {
        return Err(format!(
            "target '{name}' already has an agent enrollment attestation"
        ));
    }
    Ok(true)
}

/// Move a new or eligible legacy target into the non-routable provisioning
/// section. Schedulers and fleet readers cannot see this entry; only the
/// bootstrap agent lookup can resolve it until attestation succeeds.
pub fn stage_verified(
    document: &Value,
    name: &str,
    destination: Option<&str>,
    kind: &str,
    hostname: &str,
    takeover: bool,
) -> Result<Value, String> {
    let mut next = document.clone();
    let root = next
        .as_object_mut()
        .ok_or_else(|| "registry must be an object".to_string())?;
    let targets = root
        .get_mut("targets")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| "registry.targets: must be an array".to_string())?;
    let mut staged = if takeover {
        let index = targets
            .iter()
            .position(|target| target.get("name").and_then(Value::as_str) == Some(name))
            .ok_or_else(|| format!("target '{name}' disappeared before provisioning"))?;
        targets.remove(index)
    } else {
        json!({ "name": name, "kind": kind })
    };
    if targets
        .iter()
        .any(|target| target.get("name").and_then(Value::as_str) == Some(name))
    {
        return Err(format!("target '{name}' is already registered"));
    }
    if let Some(previous_fleet) = staged.get("fleet").cloned() {
        staged["_enrollment_previous_fleet"] = previous_fleet;
    }
    staged
        .as_object_mut()
        .ok_or_else(|| format!("target '{name}' must be an object"))?
        .remove("fleet");
    if let Some(destination) = destination {
        staged["ssh"] = Value::String(destination.to_string());
    } else {
        staged
            .as_object_mut()
            .expect("target object checked above")
            .remove("ssh");
    }
    staged["kind"] = Value::String(kind.to_string());
    staged["hostnames"] = json!([hostname]);
    staged["pinned_only"] = Value::Bool(true);
    staged["notes"] = Value::String("agent provisioning in progress".to_string());

    let provisioning = root
        .entry("provisioning_targets".to_string())
        .or_insert_with(|| json!([]))
        .as_array_mut()
        .ok_or_else(|| "registry.provisioning_targets: must be an array".to_string())?;
    if provisioning
        .iter()
        .any(|target| target.get("name").and_then(Value::as_str) == Some(name))
    {
        return Err(format!("target '{name}' is already being provisioned"));
    }
    provisioning.push(staged);
    Ok(next)
}

/// Promote one attested provisioning target into the registered target set.
/// Fleet membership becomes visible in the same validated registry write.
pub fn finalize_registration(
    document: &Value,
    name: &str,
    fleet_name: Option<&str>,
    attestation: Value,
) -> Result<Value, String> {
    let mut next = document.clone();
    let root = next
        .as_object_mut()
        .ok_or_else(|| "registry must be an object".to_string())?;
    let provisioning = root
        .get_mut("provisioning_targets")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| "registry.provisioning_targets: must be an array".to_string())?;
    let index = provisioning
        .iter()
        .position(|target| target.get("name").and_then(Value::as_str) == Some(name))
        .ok_or_else(|| format!("provisioning target '{name}' disappeared"))?;
    let mut target = provisioning.remove(index);
    let previous_fleet = target
        .as_object_mut()
        .and_then(|target| target.remove("_enrollment_previous_fleet"))
        .and_then(|value| value.as_str().map(str::to_string));
    if let Some(fleet) = fleet_name.map(str::to_string).or(previous_fleet) {
        target["fleet"] = Value::String(fleet);
    }
    target["agent_enrollment"] = attestation;
    target["notes"] = Value::String("enrolled after live agent attestation".to_string());
    let targets = root
        .get_mut("targets")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| "registry.targets: must be an array".to_string())?;
    if targets
        .iter()
        .any(|target| target.get("name").and_then(Value::as_str) == Some(name))
    {
        return Err(format!(
            "target '{name}' became registered during provisioning"
        ));
    }
    targets.push(target);
    Ok(next)
}

/// Roll back only the provisioning entry touched by enrollment, preserving
/// concurrent changes elsewhere and restoring an eligible legacy target.
pub fn rollback_registration(
    current: &Value,
    original: &Value,
    name: &str,
    takeover: bool,
) -> Result<Value, String> {
    let mut next = current.clone();
    let root = next
        .as_object_mut()
        .ok_or_else(|| "registry must be an object".to_string())?;
    let provisioning = root
        .get_mut("provisioning_targets")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| "registry.provisioning_targets: must be an array".to_string())?;
    provisioning.retain(|target| target.get("name").and_then(Value::as_str) != Some(name));
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
        let targets = root
            .get_mut("targets")
            .and_then(Value::as_array_mut)
            .ok_or_else(|| "registry.targets: must be an array".to_string())?;
        if targets
            .iter()
            .any(|target| target.get("name").and_then(Value::as_str) == Some(name))
        {
            return Err(format!("target '{name}' was concurrently registered"));
        }
        targets.push(previous);
    }
    Ok(next)
}
