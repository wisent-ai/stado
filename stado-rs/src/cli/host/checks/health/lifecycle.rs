use std::collections::BTreeMap;

use serde_json::{json, Value};

/// Read a fixed set of non-secret systemd properties for one exact unit.
///
/// The manager scope comes from the collector's own unit entry. A missing
/// scope, command failure, or timeout is unread state and returns `None`.
async fn local_systemd_properties(
    manager: &str,
    unit: &str,
    properties: &str,
    runner: &crate::deploy::Runner,
) -> Option<BTreeMap<String, String>> {
    let mut argv = vec!["/usr/bin/systemctl".to_string()];
    match manager {
        "system" => {}
        "user" => argv.push("--user".to_string()),
        _ => return None,
    }
    argv.extend([
        "show".to_string(),
        format!("--property={properties}"),
        "--".to_string(),
        unit.to_string(),
    ]);
    let mut spec = crate::deploy::CommandSpec::new(argv);
    spec.timeout = Some(std::time::Duration::from_secs(2));
    let output = runner(spec).await.ok()?;
    if !output.ok() {
        return None;
    }
    Some(
        output
            .stdout
            .lines()
            .filter_map(|line| line.split_once('='))
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect(),
    )
}

/// Reconcile a collector's non-active systemd sample with native lifecycle
/// evidence that exists at publication time.
///
/// A timer-triggered oneshot is not a continuously active service. It becomes
/// `scheduled` only when systemd proves its type, an active trigger, and either
/// an execution underway or a completed successful run. Ordinary services
/// become `active` only when their own native state is active or reloading.
/// Any unread or incomplete evidence leaves the collector's explicit
/// non-active state untouched.
pub(super) async fn refresh_local_unit_lifecycle(
    document: &mut Value,
    runner: &crate::deploy::Runner,
) {
    if !cfg!(target_os = "linux") {
        return;
    }
    let pending: Vec<(String, String, String)> = document
        .get("units")
        .and_then(Value::as_object)
        .into_iter()
        .flat_map(|units| units.iter())
        .filter_map(|(unit, entry)| {
            let fields = entry.as_object()?;
            let state = fields.get("state").and_then(Value::as_str)?;
            let manager = fields.get("manager").and_then(Value::as_str)?;
            (state != "active").then(|| (unit.clone(), state.to_string(), manager.to_string()))
        })
        .collect();

    for (unit, collector_state, manager) in pending {
        let Some(properties) = local_systemd_properties(
            &manager,
            &unit,
            "LoadState,ActiveState,Type,Result,ExecMainStatus,ExecMainStartTimestamp,TriggeredBy",
            runner,
        )
        .await
        else {
            continue;
        };
        if properties.get("LoadState").map(String::as_str) != Some("loaded") {
            continue;
        }
        let Some(native_state) = properties.get("ActiveState").map(String::as_str) else {
            continue;
        };

        let triggers: Vec<String> = properties
            .get("TriggeredBy")
            .into_iter()
            .flat_map(|value| value.split_whitespace())
            .map(str::to_string)
            .collect();
        let oneshot = properties.get("Type").map(String::as_str) == Some("oneshot");
        let scheduled_run = if oneshot && native_state == "activating" {
            Some("running")
        } else if oneshot
            && properties.get("Result").map(String::as_str) == Some("success")
            && properties.get("ExecMainStatus").map(String::as_str) == Some("0")
            && properties
                .get("ExecMainStartTimestamp")
                .is_some_and(|stamp| !stamp.is_empty() && stamp != "n/a")
            && matches!(native_state, "inactive" | "active" | "reloading")
        {
            Some("succeeded")
        } else {
            None
        };
        let mut active_trigger = None;
        if scheduled_run.is_some() && !triggers.is_empty() {
            for trigger in &triggers {
                let Some(trigger_properties) =
                    local_systemd_properties(&manager, trigger, "LoadState,ActiveState", runner)
                        .await
                else {
                    continue;
                };
                let trigger_state = trigger_properties.get("ActiveState");
                if trigger_properties.get("LoadState").map(String::as_str) == Some("loaded")
                    && trigger_state.is_some_and(|state| {
                        matches!(state.as_str(), "active" | "activating" | "reloading")
                    })
                {
                    active_trigger = trigger_state.map(|state| (trigger.clone(), state.clone()));
                    break;
                }
            }
        }

        let published_state = if active_trigger.is_some() && scheduled_run.is_some() {
            "scheduled"
        } else if !oneshot && matches!(native_state, "active" | "reloading") {
            "active"
        } else {
            continue;
        };
        let Some(fields) = document
            .get_mut("units")
            .and_then(Value::as_object_mut)
            .and_then(|units| units.get_mut(&unit))
            .and_then(Value::as_object_mut)
        else {
            continue;
        };
        fields.insert("state".to_string(), json!(published_state));
        fields.insert("collector_state".to_string(), json!(collector_state));
        fields.insert("native_state".to_string(), json!(native_state));
        if published_state == "scheduled" {
            fields.insert("service_type".to_string(), json!("oneshot"));
            fields.insert("run_state".to_string(), json!(scheduled_run));
            fields.insert("triggered_by".to_string(), json!(triggers));
            if scheduled_run == Some("succeeded") {
                fields.insert("result".to_string(), json!("success"));
                fields.insert("exec_main_status".to_string(), json!("0"));
                fields.insert(
                    "last_started_at".to_string(),
                    json!(properties.get("ExecMainStartTimestamp")),
                );
            }
            if let Some((trigger, trigger_state)) = active_trigger {
                fields.insert("active_trigger".to_string(), json!(trigger));
                fields.insert("trigger_state".to_string(), json!(trigger_state));
            }
        }
    }
}
