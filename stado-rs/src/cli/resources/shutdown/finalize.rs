//! Reconciling one draft action against observed state: already-quiet
//! resources drop out, and every survivor carries pre/postconditions and the
//! rollback that makes it reversible.

use serde_json::{json, Value};

use crate::cli::resources::model::{Action, ActionKind, Rollback};
use crate::cli::resources::planner;
use crate::cli::CmdError;

pub(super) fn finalize(mut action: Action, observed: Value) -> Result<Option<Action>, CmdError> {
    if observed.get("exists").and_then(Value::as_bool) == Some(false) {
        return Err(CmdError::click(format!(
            "shutdown resource {} does not exist",
            action.resource.reference
        )));
    }
    match action.kind {
        ActionKind::PauseScheduler => {
            let state = required_string(&observed, "state", &action)?;
            if state == "PAUSED" {
                return Ok(None);
            }
            if state != "ENABLED" {
                return Err(CmdError::click(format!(
                    "Scheduler job {} is in unsupported state {state}",
                    action.resource.reference
                )));
            }
            action.preconditions = vec![planner::condition("state", json!(state))];
            action.postconditions = vec![planner::condition("state", json!("PAUSED"))];
            action.rollback = Some(Rollback {
                kind: ActionKind::ResumeScheduler,
                parameters: json!({}),
                preconditions: action.postconditions.clone(),
                postconditions: vec![planner::condition("state", json!(state))],
            });
        }
        ActionKind::ResizeManagedInstanceGroup => {
            let target = observed
                .get("target_size")
                .and_then(Value::as_i64)
                .ok_or_else(|| CmdError::click("managed group has no target size"))?;
            if target == i64::default() {
                return Ok(None);
            }
            action.parameters["target_size"] = json!(i64::default());
            action.preconditions = vec![planner::condition("target_size", json!(target))];
            action.postconditions = vec![planner::condition("target_size", json!(i64::default()))];
            action.rollback = Some(Rollback {
                kind: ActionKind::ResizeManagedInstanceGroup,
                parameters: json!({"target_size": target, "scope": action.parameters["scope"]}),
                preconditions: action.postconditions.clone(),
                postconditions: vec![planner::condition("target_size", json!(target))],
            });
        }
        ActionKind::StopInstance => {
            if observed.get("has_local_ssd").and_then(Value::as_bool) == Some(true) {
                return Err(CmdError::click(format!(
                    "refusing {}: Local SSD would require destructive discard",
                    action.resource.reference
                )));
            }
            let status = required_string(&observed, "status", &action)?;
            if status == "TERMINATED" {
                return Ok(None);
            }
            if status != "RUNNING" {
                return Err(CmdError::click(format!(
                    "instance {} is in unsupported state {status}",
                    action.resource.reference
                )));
            }
            action.preconditions = vec![
                planner::condition("status", json!(status)),
                planner::condition("has_local_ssd", json!(false)),
            ];
            action.postconditions = vec![planner::condition("status", json!("TERMINATED"))];
            action.rollback = Some(Rollback {
                kind: ActionKind::StartInstance,
                parameters: json!({}),
                preconditions: action.postconditions.clone(),
                postconditions: vec![planner::condition("status", json!("RUNNING"))],
            });
        }
        ActionKind::SuspendCloudSql => {
            let policy = required_string(&observed, "activation_policy", &action)?;
            if policy == "NEVER" {
                return Ok(None);
            }
            action.preconditions = vec![planner::condition("activation_policy", json!(policy))];
            action.postconditions = vec![planner::condition("activation_policy", json!("NEVER"))];
            action.rollback = Some(Rollback {
                kind: ActionKind::RestoreCloudSql,
                parameters: json!({"activation_policy": policy}),
                preconditions: action.postconditions.clone(),
                postconditions: vec![planner::condition("activation_policy", json!(policy))],
            });
        }
        _ => {
            return Err(CmdError::click(
                "shutdown planner received a destructive or unsupported action",
            ))
        }
    }
    for field in [
        "resource_id",
        "creation_timestamp",
        "fingerprint",
        "metadata_fingerprint",
        "etag",
        "settings_version",
    ] {
        if let Some(value) = observed.get(field).filter(|value| !value.is_null()) {
            action
                .preconditions
                .push(planner::condition(field, value.clone()));
        }
    }
    Ok(Some(action))
}

fn required_string<'a>(
    observed: &'a Value,
    key: &str,
    action: &Action,
) -> Result<&'a str, CmdError> {
    observed.get(key).and_then(Value::as_str).ok_or_else(|| {
        CmdError::click(format!(
            "resource {} has no {key}",
            action.resource.reference
        ))
    })
}
