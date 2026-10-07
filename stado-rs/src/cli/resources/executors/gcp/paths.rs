//! Resource paths: every URL an action can reach is built here from the
//! action's own scope, project and location, never from plan text.

use serde_json::Value;

use crate::cli::resources::model::Action;
use crate::cli::CmdError;

pub(super) fn disk_path(action: &Action) -> Result<String, CmdError> {
    let parent = zonal_or_regional(action)?;
    Ok(format!(
        "/projects/{}/{parent}/{}/disks/{}",
        project(action)?,
        location(action)?,
        action.resource.name
    ))
}

pub(super) fn address_path(action: &Action) -> Result<String, CmdError> {
    Ok(match scope(action)? {
        "global" => format!(
            "/projects/{}/global/addresses/{}",
            project(action)?,
            action.resource.name
        ),
        "region" => format!(
            "/projects/{}/regions/{}/addresses/{}",
            project(action)?,
            location(action)?,
            action.resource.name
        ),
        other => return Err(unsupported_scope(action, other, "global or region")),
    })
}

pub(super) fn mig_path(action: &Action) -> Result<String, CmdError> {
    let parent = zonal_or_regional(action)?;
    Ok(format!(
        "/projects/{}/{parent}/{}/instanceGroupManagers/{}",
        project(action)?,
        location(action)?,
        action.resource.name
    ))
}

pub(super) fn reservation_path(action: &Action) -> Result<String, CmdError> {
    Ok(format!(
        "/projects/{}/zones/{}/reservations/{}",
        project(action)?,
        location(action)?,
        action.resource.name
    ))
}

/// The Compute Engine collection a zonal or regional resource lives under,
/// from the scope the plan recorded for it.
pub(super) fn zonal_or_regional(action: &Action) -> Result<&'static str, CmdError> {
    match scope(action)? {
        "zone" => Ok("zones"),
        "region" => Ok("regions"),
        other => Err(unsupported_scope(action, other, "zone or region")),
    }
}

/// The scope the planner recorded with the action. A plan without one names
/// no collection, so the action is refused rather than sent to a guessed one.
fn scope(action: &Action) -> Result<&str, CmdError> {
    action
        .parameters
        .get("scope")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            CmdError::click(format!(
                "action {} records no scope, so its Compute Engine collection is unknown",
                action.id
            ))
            .stating(crate::primitives::failure::FailureCode::Config)
        })
}

fn unsupported_scope(action: &Action, scope: &str, expected: &str) -> CmdError {
    CmdError::click(format!(
        "action {} records scope {scope:?}; this resource lives in a {expected} collection",
        action.id
    ))
    .stating(crate::primitives::failure::FailureCode::Config)
}

fn project(action: &Action) -> Result<&str, CmdError> {
    action.resource.project.as_deref().ok_or_else(|| {
        CmdError::click(format!("action {} has no project", action.id))
            .stating(crate::primitives::failure::FailureCode::Config)
    })
}

pub(super) fn location(action: &Action) -> Result<&str, CmdError> {
    action.resource.location.as_deref().ok_or_else(|| {
        CmdError::click(format!("action {} has no location", action.id))
            .stating(crate::primitives::failure::FailureCode::Config)
    })
}

pub(super) fn parameter_str<'a>(
    value: &'a Value,
    key: &str,
    action: &Action,
) -> Result<&'a str, CmdError> {
    value.get(key).and_then(Value::as_str).ok_or_else(|| {
        CmdError::click(format!("action {} has no {key}", action.id))
            .stating(crate::primitives::failure::FailureCode::Config)
    })
}

pub(super) fn json_u64(value: &Value) -> Option<u64> {
    value
        .as_u64()
        .or_else(|| value.as_str().and_then(|text| text.parse().ok()))
}
