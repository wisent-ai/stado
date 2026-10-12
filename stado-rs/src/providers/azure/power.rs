//! What Azure says about a resource's life: a VM's power code and any
//! resource's provisioning state, as Azure Resource Manager documents them,
//! and what each means for the VM's liveness and its lifecycle phase.

use serde::Deserialize;
use serde_json::Value;

use crate::providers::{Lifecycle, LifecyclePhase};

/// What an Azure resource's state word means: a VM's power code first, a
/// provisioning state otherwise. A word outside both lists is unknown.
pub fn resource_phase(word: &str) -> LifecyclePhase {
    let word = Value::String(word.to_string());
    if let Ok(power) = PowerState::deserialize(&word) {
        return power.phase();
    }
    match ProvisioningState::deserialize(&word) {
        Ok(
            ProvisioningState::Failed | ProvisioningState::Deleting | ProvisioningState::Canceled,
        ) => LifecyclePhase::Ended,
        Ok(_) => LifecyclePhase::Running,
        Err(_) => LifecyclePhase::Unknown,
    }
}

/// First `PowerState/...` code of the instanceView ("running",
/// "deallocated", ...), None when absent.
pub fn power_state(vm: &Value) -> Option<String> {
    let statuses = vm
        .get("properties")?
        .get("instanceView")?
        .get("statuses")?
        .as_array()?;
    for status in statuses {
        if let Some(code) = status.get("code").and_then(Value::as_str) {
            if let Some(state) = code.strip_prefix("PowerState/") {
                return Some(state.to_string());
            }
        }
    }
    None
}

/// A VM's `properties.provisioningState`, as Azure Resource Manager
/// documents it. A value outside the list parses to nothing, and an
/// unreadable state is not alive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum ProvisioningState {
    Creating,
    Updating,
    Succeeded,
    Failed,
    Deleting,
    Canceled,
    Migrating,
}

/// A VM's `PowerState/<code>` from its instance view, as Azure documents
/// the codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PowerState {
    Starting,
    Running,
    Stopping,
    Stopped,
    Deallocating,
    Deallocated,
    Unknown,
}

impl PowerState {
    /// What the power code means for the VM's life.
    fn phase(self) -> LifecyclePhase {
        match self {
            Self::Starting | Self::Running => LifecyclePhase::Running,
            Self::Stopping | Self::Stopped | Self::Deallocating | Self::Deallocated => {
                LifecyclePhase::Stopped
            }
            Self::Unknown => LifecyclePhase::Unknown,
        }
    }
}

/// The VM's lifecycle from its power code, `None` when it has none yet.
pub fn lifecycle(vm: &Value) -> Option<Lifecycle> {
    let word = power_state(vm)?;
    let phase = PowerState::deserialize(Value::String(word.clone()))
        .map_or(LifecyclePhase::Unknown, PowerState::phase);
    Some(Lifecycle { word, phase })
}

/// Python `instance_exists` mapping: a VM being created or updated, or
/// created, whose power is coming up or on, is the closest analogue to GCE
/// RUNNING/STAGING/PROVISIONING. A VM mid-update still consumes GPU quota
/// and is not requeued; mid-create it has no power state yet and is alive.
pub fn vm_is_alive(vm: &Value) -> bool {
    let provisioning = ProvisioningState::deserialize(&vm["properties"]["provisioningState"]).ok();
    let starting = matches!(
        provisioning,
        Some(ProvisioningState::Creating | ProvisioningState::Updating)
    );
    // No power code yet is mid-create; a code outside the documented list
    // is not a power that is coming up or on.
    match power_state(vm) {
        None => starting,
        Some(code) => {
            matches!(
                PowerState::deserialize(Value::String(code)),
                Ok(PowerState::Running | PowerState::Starting)
            ) && (starting || provisioning == Some(ProvisioningState::Succeeded))
        }
    }
}
