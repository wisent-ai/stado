//! Resource paths: every URL an action can reach is built here from the
//! action's own scope, project and location, never from plan text.

use serde_json::Value;

use crate::cli::resources::model::Action;
use crate::cli::CmdError;

pub(super) fn disk_path(action: &Action) -> Result<String, CmdError> {
    Ok(match scope(action) {
        "region" => format!(
            "/projects/{}/regions/{}/disks/{}",
            project(action)?,
            location(action)?,
            action.resource.name
        ),
        _ => format!(
            "/projects/{}/zones/{}/disks/{}",
            project(action)?,
            location(action)?,
            action.resource.name
        ),
    })
}

pub(super) fn address_path(action: &Action) -> Result<String, CmdError> {
    Ok(if scope(action) == "global" {
        format!(
            "/projects/{}/global/addresses/{}",
            project(action)?,
            action.resource.name
        )
    } else {
        format!(
            "/projects/{}/regions/{}/addresses/{}",
            project(action)?,
            location(action)?,
            action.resource.name
        )
    })
}

pub(super) fn mig_path(action: &Action) -> Result<String, CmdError> {
    Ok(match scope(action) {
        "region" => format!(
            "/projects/{}/regions/{}/instanceGroupManagers/{}",
            project(action)?,
            location(action)?,
            action.resource.name
        ),
        _ => format!(
            "/projects/{}/zones/{}/instanceGroupManagers/{}",
            project(action)?,
            location(action)?,
            action.resource.name
        ),
    })
}

pub(super) fn reservation_path(action: &Action) -> Result<String, CmdError> {
    Ok(format!(
        "/projects/{}/zones/{}/reservations/{}",
        project(action)?,
        location(action)?,
        action.resource.name
    ))
}

pub(super) fn scope(action: &Action) -> &str {
    action
        .parameters
        .get("scope")
        .and_then(Value::as_str)
        .unwrap_or("zone")
}

fn project(action: &Action) -> Result<&str, CmdError> {
    action
        .resource
        .project
        .as_deref()
        .ok_or_else(|| CmdError::click(format!("action {} has no project", action.id)))
}

pub(super) fn location(action: &Action) -> Result<&str, CmdError> {
    action
        .resource
        .location
        .as_deref()
        .ok_or_else(|| CmdError::click(format!("action {} has no location", action.id)))
}

pub(super) fn parameter_str<'a>(
    value: &'a Value,
    key: &str,
    action: &Action,
) -> Result<&'a str, CmdError> {
    value
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| CmdError::click(format!("action {} has no {key}", action.id)))
}

pub(super) fn json_u64(value: &Value) -> Option<u64> {
    value
        .as_u64()
        .or_else(|| value.as_str().and_then(|text| text.parse().ok()))
}
