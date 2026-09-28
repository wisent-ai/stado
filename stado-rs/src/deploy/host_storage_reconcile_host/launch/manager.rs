//! What the native manager says about the transaction's worker unit, and
//! whether the worker has recorded itself as the owner bound to it.

use std::fs;
use std::process::{Command, Stdio};

use regex::Regex;
use serde_json::{json, Map, Value};

use super::Launch;

fn capture<'a>(pattern: &str, text: &'a str) -> Option<&'a str> {
    Regex::new(pattern)
        .ok()?
        .captures(text)?
        .get(1)
        .map(|found| found.as_str())
}

fn quiet_sudo(arguments: &[&str]) -> Option<String> {
    let output = Command::new("/usr/bin/sudo")
        .arg("-n")
        .args(arguments)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}

fn launchd_state(label: &str) -> Value {
    let Some(printed) = quiet_sudo(&["/bin/launchctl", "print", &format!("system/{label}")]) else {
        return json!({
            "manager": "launchd", "service": label, "domain": "system",
            "loaded": false, "active": false, "starting": false, "pid": null, "state": null,
        });
    };
    let pid: Option<u64> =
        capture(r"(?m)^\s*pid = ([1-9][0-9]*)\s*$", &printed).and_then(|pid| pid.parse().ok());
    let state = capture(r"(?m)^\s*state = (.+?)\s*$", &printed).map(str::trim);
    let completed = Regex::new(r"(?m)^\s*last exit code = -?[0-9]+\s*$")
        .is_ok_and(|pattern| pattern.is_match(&printed));
    let runs = capture(r"(?m)^\s*runs = ([1-9][0-9]*)\s*$", &printed).is_some();
    let lowered = state.unwrap_or_default().to_lowercase();
    let terminal =
        lowered == "exited" || lowered == "not running" || (pid.is_none() && completed && runs);
    json!({
        "manager": "launchd", "service": label, "domain": "system", "loaded": true,
        "active": pid.is_some(), "starting": pid.is_none() && !terminal,
        "pid": pid, "state": state,
    })
}

fn systemd_state(label: &str) -> Value {
    let unit = format!("{label}.service");
    let mut properties = Map::new();
    let property_names = "--property=LoadState,ActiveState,SubState,MainPID";
    if let Some(shown) = quiet_sudo(&["/bin/systemctl", "show", property_names, &unit]) {
        for line in shown.lines() {
            if let Some((key, value)) = line.split_once('=') {
                properties.insert(key.to_string(), json!(value));
            }
        }
    }
    let property = |key: &str| properties.get(key).and_then(Value::as_str);
    let pid = property("MainPID")
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|pid| *pid > 0);
    let active_state = property("ActiveState");
    let active =
        pid.is_some() || matches!(active_state, Some("active" | "activating" | "reloading"));
    json!({
        "manager": "systemd", "service": unit,
        "loaded": property("LoadState") == Some("loaded"),
        "active": active, "starting": active_state == Some("activating"),
        "pid": pid, "load_state": property("LoadState"),
        "active_state": active_state, "sub_state": property("SubState"),
    })
}

pub(super) fn manager_state(launch: &Launch) -> Result<Value, String> {
    if cfg!(target_os = "macos") {
        Ok(launchd_state(&launch.label))
    } else if cfg!(target_os = "linux") {
        Ok(systemd_state(&launch.label))
    } else {
        Err("native reconciliation worker requires Darwin launchd or Linux systemd".to_string())
    }
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
