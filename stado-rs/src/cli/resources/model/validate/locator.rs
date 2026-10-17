//! Whether an action addresses a resource its kind can actually act on: the
//! provider, the resource type, the scope the executor will use, and the
//! shape of every path component named in the locator.

use serde_json::Value;

use crate::cli::CmdError;

use super::super::{Action, ActionKind, ProviderKind};

pub(super) fn validate_action_locator(action: &Action) -> Result<(), CmdError> {
    let valid = match action.kind {
        ActionKind::DeleteInstance => {
            action.resource.resource_type == "agent-vm"
                && matches!(
                    action.resource.provider,
                    ProviderKind::Gcp
                        | ProviderKind::Azure
                        | ProviderKind::Aws
                        | ProviderKind::Boat
                )
        }
        ActionKind::SnapshotDisk | ActionKind::DeleteDisk => {
            valid_gcp_locator(action, "persistent-disk", &["zone", "region"])
                && action
                    .parameters
                    .get("snapshot_name")
                    .and_then(Value::as_str)
                    .is_some_and(valid_component)
        }
        ActionKind::ReleaseAddress => {
            valid_gcp_locator(action, "static-address", &["global", "region"])
        }
        ActionKind::DeleteManagedInstanceGroup => {
            valid_gcp_locator(action, "managed-instance-group", &["zone", "region"])
        }
        // A plan resizes a managed group only to empty: no instance left.
        ActionKind::ResizeManagedInstanceGroup => {
            valid_gcp_locator(action, "managed-instance-group", &["zone", "region"])
                && action
                    .parameters
                    .get("target_size")
                    .and_then(Value::as_u64)
                    .is_some_and(|size| std::num::NonZeroU64::new(size).is_none())
        }
        ActionKind::ReleaseReservation => {
            valid_gcp_locator(action, "compute-reservation", &["zone"])
        }
        ActionKind::DisableStorageBackup => {
            action.resource.provider == ProviderKind::Stado
                && action.resource.resource_type == "storage-backup"
                && action
                    .parameters
                    .get("previous")
                    .is_some_and(Value::is_object)
        }
        ActionKind::PauseScheduler => valid_gcp_locator(action, "scheduler-job", &["region"]),
        ActionKind::StopInstance => {
            (action.resource.resource_type == "agent-vm"
                && matches!(
                    action.resource.provider,
                    ProviderKind::Gcp | ProviderKind::Azure | ProviderKind::Aws
                ))
                || valid_gcp_locator(action, "instance", &["zone"])
        }
        ActionKind::StartInstance => {
            action.resource.resource_type == "agent-vm"
                && matches!(
                    action.resource.provider,
                    ProviderKind::Gcp | ProviderKind::Azure | ProviderKind::Aws
                )
        }
        ActionKind::SuspendCloudSql => valid_gcp_locator(action, "cloud-sql-instance", &["global"]),
        rollback => {
            return Err(CmdError::refused(format!(
                "rollback-only action kind {rollback:?} cannot appear in a plan"
            )))
        }
    };
    if !valid {
        return Err(CmdError::refused(format!(
            "action {} has a provider, type, scope, or locator incompatible with {:?}",
            action.id, action.kind
        )));
    }
    Ok(())
}

fn valid_gcp_locator(action: &Action, resource_type: &str, scopes: &[&str]) -> bool {
    // An action without a textual scope names no place, so no locator fits.
    let Some(scope) = action.parameters.get("scope").and_then(Value::as_str) else {
        return false;
    };
    let needs_location = scope != "global";
    action.resource.provider == ProviderKind::Gcp
        && action.resource.resource_type == resource_type
        && action
            .resource
            .project
            .as_deref()
            .is_some_and(valid_component)
        && valid_component(&action.resource.name)
        && scopes.contains(&scope)
        && (!needs_location
            || action
                .resource
                .location
                .as_deref()
                .is_some_and(valid_component))
}

pub(super) fn valid_component(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= u8::MAX as usize
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}
