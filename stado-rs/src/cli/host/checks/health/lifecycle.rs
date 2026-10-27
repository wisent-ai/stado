use std::collections::BTreeMap;

use serde_json::{json, Value};

use crate::deploy::service::{SystemdActiveState, SystemdManager};

/// Read a fixed set of non-secret systemd properties for one exact unit.
///
/// The manager scope comes from the collector's own unit entry. A command
/// that could not run or failed is unread state, returned with its cause so
/// the beacon can say why the collector's state was left as it was.
async fn local_systemd_properties(
    manager: SystemdManager,
    unit: &str,
    properties: &str,
    runner: &crate::deploy::Runner,
) -> Result<BTreeMap<String, String>, String> {
    let mut argv = vec!["/usr/bin/systemctl".to_string()];
    if manager == SystemdManager::User {
        argv.push("--user".to_string());
    }
    argv.extend([
        "show".to_string(),
        format!("--property={properties}"),
        "--".to_string(),
        unit.to_string(),
    ]);
    let spec = crate::deploy::CommandSpec::new(argv);
    let output = runner(spec)
        .await
        .map_err(|error| format!("systemctl show {unit} could not run: {error}"))?;
    if !output.succeeded() {
        return Err(format!(
            "systemctl show {unit} failed: {}",
            output.stderr.trim()
        ));
    }
    Ok(output
        .stdout
        .lines()
        .filter_map(|line| line.split_once('='))
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect())
}

/// The beacon's own entry for `unit`, when the document carries one as an
/// object: the one place the reconciliation writes a unit's fields.
fn unit_fields<'a>(
    document: &'a mut Value,
    unit: &str,
) -> Option<&'a mut serde_json::Map<String, Value>> {
    document
        .get_mut("units")
        .and_then(Value::as_object_mut)
        .and_then(|units| units.get_mut(unit))
        .and_then(Value::as_object_mut)
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
    let pending: Vec<(String, String, Result<SystemdManager, String>)> = document
        .get("units")
        .and_then(Value::as_object)
        .into_iter()
        .flat_map(|units| units.iter())
        .filter_map(|(unit, entry)| {
            let fields = entry.as_object()?;
            let state = fields.get("state").and_then(Value::as_str)?;
            // The manager is carried as a result so a word that names no
            // manager is said on the unit below, not dropped here.
            let manager = match fields.get("manager").and_then(Value::as_str) {
                Some(word) => SystemdManager::parse(word),
                None => Err("the collector's sample names no manager".to_string()),
            };
            (state != crate::deploy::service::STATE_ACTIVE)
                .then(|| (unit.clone(), state.to_string(), manager))
        })
        .collect();

    for (unit, collector_state, manager) in pending {
        // A sample whose manager word names no systemd manager cannot be
        // asked about; the unit says so, as it does for an unread state.
        let manager = match manager {
            Ok(manager) => manager,
            Err(unread) => {
                if let Some(fields) = unit_fields(document, &unit) {
                    fields.insert("lifecycle_unread".to_string(), json!(unread));
                }
                continue;
            }
        };
        // An unread native state leaves the collector's state as it was, and
        // now says so on the unit, instead of leaving no trace that the
        // reconciliation was attempted and could not read systemd.
        let properties = match local_systemd_properties(
            manager,
            &unit,
            "LoadState,ActiveState,Type,Result,ExecMainStatus,ExecMainStartTimestamp,TriggeredBy",
            runner,
        )
        .await
        {
            Ok(properties) => properties,
            Err(unread) => {
                if let Some(fields) = unit_fields(document, &unit) {
                    fields.insert("lifecycle_unread".to_string(), json!(unread));
                }
                continue;
            }
        };
        if properties.get("LoadState").map(String::as_str) != Some("loaded") {
            continue;
        }
        let Some(native_word) = properties.get("ActiveState").map(String::as_str) else {
            continue;
        };
        let native_state = match SystemdActiveState::parse(native_word) {
            Ok(state) => state,
            Err(unknown) => {
                if let Some(fields) = unit_fields(document, &unit) {
                    fields.insert("lifecycle_unread".to_string(), json!(unknown));
                }
                continue;
            }
        };

        let triggers: Vec<String> = properties
            .get("TriggeredBy")
            .into_iter()
            .flat_map(|value| value.split_whitespace())
            .map(str::to_string)
            .collect();
        let oneshot = properties.get("Type").map(String::as_str) == Some("oneshot");
        let scheduled_run = if oneshot && native_state == SystemdActiveState::Activating {
            Some("running")
        } else if oneshot
            && properties.get("Result").map(String::as_str) == Some("success")
            && properties.get("ExecMainStatus").map(String::as_str) == Some("0")
            && properties
                .get("ExecMainStartTimestamp")
                .is_some_and(|stamp| !stamp.is_empty() && stamp != "n/a")
            && native_state.settled()
        {
            Some("succeeded")
        } else {
            None
        };
        let mut active_trigger = None;
        // A trigger whose state cannot be read is said on the unit beside
        // the others, never dropped: dropped, a timer systemd could not
        // answer for left the unit unscheduled with no trace why.
        let mut triggers_unread = Vec::new();
        if scheduled_run.is_some() && !triggers.is_empty() {
            for trigger in &triggers {
                let trigger_properties = match local_systemd_properties(
                    manager,
                    trigger,
                    "LoadState,ActiveState",
                    runner,
                )
                .await
                {
                    Ok(properties) => properties,
                    Err(unread) => {
                        triggers_unread.push(unread);
                        continue;
                    }
                };
                let trigger_state = trigger_properties.get("ActiveState");
                // A trigger state systemd does not write is said beside the
                // unread triggers; it never counts as an active trigger.
                let trigger_up = match trigger_state.map(|state| SystemdActiveState::parse(state)) {
                    Some(Ok(state)) => state.up_or_starting(),
                    Some(Err(unknown)) => {
                        triggers_unread.push(format!("{trigger}: {unknown}"));
                        false
                    }
                    None => false,
                };
                if trigger_properties.get("LoadState").map(String::as_str) == Some("loaded")
                    && trigger_up
                {
                    active_trigger = trigger_state.map(|state| (trigger.clone(), state.clone()));
                    break;
                }
            }
        }
        if !triggers_unread.is_empty() {
            if let Some(fields) = unit_fields(document, &unit) {
                fields.insert("triggers_unread".to_string(), json!(triggers_unread));
            }
        }

        let published_state = if active_trigger.is_some() && scheduled_run.is_some() {
            "scheduled"
        } else if !oneshot && native_state.up() {
            "active"
        } else {
            continue;
        };
        let Some(fields) = unit_fields(document, &unit) else {
            continue;
        };
        fields.insert("state".to_string(), json!(published_state));
        fields.insert("collector_state".to_string(), json!(collector_state));
        fields.insert("native_state".to_string(), json!(native_word));
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
