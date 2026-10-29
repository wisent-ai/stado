//! Safe repair of legacy registry entries that declared a machine without a
//! communication channel or any proof of contact.

use crate::cli::fleet::enroll::failures::{
    entry_summary, ALREADY_HAS_BEACON, ALREADY_REGISTERED, BEACON_UNREADABLE,
    PLATFORM_DECLARATION_MALFORMED, PLATFORM_MISMATCH, TARGETS_MALFORMED,
};
use crate::cli::CmdError;
use crate::monitor::host_health::HostHealthError;
use crate::queue::JobStorage;
use serde_json::{json, Value};

fn targets_missing(document: &Value, name: &str) -> CmdError {
    let held = document
        .get("targets")
        .map_or_else(|| "nothing".to_string(), Value::to_string);
    CmdError::declared(&TARGETS_MALFORMED, &[("name", name), ("targets", &held)])
}

fn target_index(document: &Value, name: &str) -> Result<Option<usize>, CmdError> {
    let targets = document
        .get("targets")
        .and_then(Value::as_array)
        .ok_or_else(|| targets_missing(document, name))?;
    Ok(targets
        .iter()
        .position(|target| target.get("name").and_then(Value::as_str) == Some(name)))
}

/// Refuse takeover unless an existing target is exactly the unverified legacy
/// shape: no channel and no health beacon. Store failures fail closed, with
/// the class the beacon read stated.
pub async fn allow_takeover(document: &Value, name: &str) -> Result<bool, CmdError> {
    let Some(index) = target_index(document, name)? else {
        return Ok(false);
    };
    let target = &document["targets"][index];
    if target.get("ssh").is_some_and(|value| !value.is_null()) {
        return Err(CmdError::declared(
            &ALREADY_REGISTERED,
            &[("name", name), ("entry", &entry_summary(target))],
        ));
    }

    let unreadable = |error: CmdError| {
        CmdError::declared(
            &BEACON_UNREADABLE,
            &[("name", name), ("answer", &error.to_string())],
        )
    };
    let store = JobStorage::new()
        .await
        .map_err(|error| unreadable(error.into()))?;
    match crate::monitor::host_health::load_host_health(&store, name).await {
        Err(HostHealthError::NoBeacon { .. }) => Ok(true),
        Ok(_) => Err(CmdError::declared(&ALREADY_HAS_BEACON, &[("name", name)])),
        Err(error) => Err(unreadable(CmdError::from(error))),
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
) -> Result<Value, CmdError> {
    let mut next = document.clone();
    let targets = next
        .get_mut("targets")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| targets_missing(document, name))?;

    if let Some(target) = targets
        .iter_mut()
        .find(|target| target.get("name").and_then(Value::as_str) == Some(name))
    {
        if !takeover {
            return Err(CmdError::declared(
                &ALREADY_REGISTERED,
                &[("name", name), ("entry", &entry_summary(target))],
            ));
        }
        // An undeclared or empty platform is the legacy declaration this
        // repairs; one declared as anything but text is not a platform, and
        // read as undeclared it would be overwritten without a word.
        match target.get("release_platform") {
            None | Some(Value::Null) => {}
            Some(Value::String(declared))
                if declared.is_empty() || declared == release_platform => {}
            Some(Value::String(declared)) => {
                return Err(CmdError::declared(
                    &PLATFORM_MISMATCH,
                    &[
                        ("name", name),
                        ("ssh", destination),
                        ("declared", declared),
                        ("platform", release_platform),
                    ],
                ));
            }
            Some(other) => {
                return Err(CmdError::declared(
                    &PLATFORM_DECLARATION_MALFORMED,
                    &[
                        ("name", name),
                        ("declared", &other.to_string()),
                        ("platform", release_platform),
                    ],
                ));
            }
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
) -> Result<Value, CmdError> {
    let mut next = current.clone();
    let targets = next
        .get_mut("targets")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| targets_missing(current, name))?;
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
            .ok_or_else(|| {
                CmdError::declaration(format!("original target '{name}' disappeared"))
            })?;
        let target = targets
            .iter_mut()
            .find(|target| target.get("name").and_then(Value::as_str) == Some(name))
            .ok_or_else(|| {
                CmdError::refused(format!("target '{name}' disappeared before rollback"))
            })?;
        *target = previous;
    } else {
        targets.retain(|target| target.get("name").and_then(Value::as_str) != Some(name));
    }
    Ok(next)
}
