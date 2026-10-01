//! What the host process says about the transaction's worker, and whether
//! the worker has recorded itself as the owner bound to that process.

use std::fs;

use serde_json::{json, Value};

use super::Launch;
use crate::release_agent::rollout::serving::control;

/// The host process's view of the worker, in the shape the worker records
/// as its manager: `service` is the host's one Stado unit, `pid` the
/// worker's own pid. No host process answering is `loaded: false`.
pub(super) fn manager_state(launch: &Launch) -> Result<Value, String> {
    let Some((host_pid, worker)) = control::inspect_transaction_blocking(None, launch.transaction)?
    else {
        return Ok(json!({
            "manager": "stado", "service": launch.label,
            "loaded": false, "active": false, "starting": false, "pid": null, "host_pid": null,
            "state": "the host process is not running",
        }));
    };
    Ok(match worker {
        Some(worker) => json!({
            "manager": "stado", "service": launch.label, "loaded": true,
            "active": worker.running, "starting": false,
            "pid": worker.pid, "host_pid": host_pid,
            "state": worker.exit.unwrap_or_else(|| "running".to_string()),
        }),
        None => json!({
            "manager": "stado", "service": launch.label, "loaded": true,
            "active": false, "starting": false, "pid": null, "host_pid": host_pid,
            "state": "no worker of this transaction",
        }),
    })
}

pub(super) fn running(state: &Value) -> bool {
    state["active"].as_bool() == Some(true) || state["starting"].as_bool() == Some(true)
}

/// A JSON document at `path`, or none when nothing is there.
pub(super) fn read_json(path: &str) -> Result<Option<Value>, String> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("cannot inspect {path}: {error}")),
        Ok(info) if !info.file_type().is_file() => {
            return Err(format!("{path} is not a regular file"));
        }
        Ok(_) => {}
    }
    let bytes = fs::read(path).map_err(|error| format!("cannot read {path}: {error}"))?;
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|error| format!("{path} is not JSON: {error}"))
}

/// The owner the worker recorded, when it names this manager's live process.
pub(super) fn manager_bound_owner(launch: &Launch, state: &Value) -> Result<Option<Value>, String> {
    let Some(mut owner) = read_json(&launch.owner_path)?.filter(Value::is_object) else {
        return Ok(None);
    };
    let native = &owner["native_manager"];
    let owner_pid = owner["pid"].as_u64().unwrap_or(0);
    let bound = owner["schema"].as_str() == Some("stado.storage-root-owner.v1")
        && owner["transaction"].as_str() == Some(launch.transaction)
        && owner["status"].as_str() == Some("executing")
        && native.is_object()
        && native.get("service") == state.get("service")
        && Some(owner_pid) == state["pid"].as_u64()
        && native.get("pid") == state.get("pid");
    if !bound {
        return Ok(None);
    }
    if let Some(fields) = owner.as_object_mut() {
        fields.remove("token");
    }
    Ok(Some(owner))
}

/// The recorded launch intent, with the manager's current view of the unit.
pub(super) fn launch_observation(launch: &Launch, state: &Value) -> Result<Value, String> {
    let Some(mut intent) = read_json(&launch.intent_path)?
        .filter(|intent| intent["transaction"].as_str() == Some(launch.transaction))
    else {
        return Err("active native worker has no recorded launch intent".to_string());
    };
    if let Some(fields) = intent.as_object_mut() {
        fields.insert("native_manager".to_string(), state.clone());
        fields.remove("worker_arguments");
    }
    Ok(intent)
}

/// Report the worker already running, when it runs the requested action.
pub(super) fn acknowledge_owner(launch: &Launch, observation: &Value) -> Result<(), String> {
    let action = observation["action"].as_str();
    let forward = |action: Option<&str>| matches!(action, Some("run" | "resume"));
    if action != Some(launch.action.as_str())
        && !(forward(action) && forward(Some(launch.action.as_str())))
    {
        return Err(format!(
            "native reconciliation is already executing {}; cannot accept {}",
            action.unwrap_or("None"),
            launch.action
        ));
    }
    println!("STADO_RECONCILE_OWNER\t{observation}");
    Ok(())
}

/// The owner already running, or the recorded intent, acknowledged.
pub(super) fn acknowledge_running(launch: &Launch, state: &Value) -> Result<(), String> {
    let observation = match manager_bound_owner(launch, state)? {
        Some(owner) => owner,
        None => launch_observation(launch, state)?,
    };
    acknowledge_owner(launch, &observation)
}
