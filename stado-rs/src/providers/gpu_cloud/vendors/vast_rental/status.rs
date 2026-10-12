//! A Vast.ai instance's lifecycle from its container status and the
//! renter's intention.

use serde::Deserialize;
use serde_json::Value;

use super::VENDOR;
use crate::providers::gpu_cloud::api::{GpuCloudError, MachineState};

/// An instance's container status (`actual_status`), as Vast.ai lists it.
#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
enum Actual {
    Loading,
    Created,
    Creating,
    Running,
    Stopped,
    Exited,
    Offline,
    Unknown,
}

/// What the renter asked an instance to be (`intended_status`); any other
/// intention is not a stop.
#[derive(Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum Intended {
    Stopped,
    #[serde(other)]
    Other,
}

/// One instance's lifecycle from `actual_status` (the container) read
/// against `intended_status` (what the renter asked for). Vast.ai documents
/// that an instance whose container is `exited`, `offline` or `unknown`
/// while it is meant to run never reaches `running`, so that is a failure.
pub(super) fn state(operation: &str, instance: &Value) -> Result<MachineState, GpuCloudError> {
    let stopped_on_purpose = instance.get("intended_status").is_some_and(|value| {
        Intended::deserialize(value).is_ok_and(|intended| intended == Intended::Stopped)
    });
    let Some(raw) = instance
        .get("actual_status")
        .filter(|value| !value.is_null())
    else {
        return Ok(MachineState::Provisioning);
    };
    let actual = Actual::deserialize(raw).map_err(|error| {
        GpuCloudError::response(
            VENDOR,
            operation,
            format!("instance actual_status {raw} is not one this adapter reads: {error}"),
        )
    })?;
    Ok(match actual {
        Actual::Loading | Actual::Created | Actual::Creating => MachineState::Provisioning,
        Actual::Running => MachineState::Running,
        Actual::Stopped => MachineState::Stopped,
        Actual::Exited if stopped_on_purpose => MachineState::Stopped,
        Actual::Exited | Actual::Offline | Actual::Unknown => MachineState::Failed,
    })
}
