//! Whether the undo an action carries would really put the resource back:
//! the pairing of forward and reverse kind, and the parameters the reverse
//! side has to agree on with the action it undoes.

use serde_json::Value;

use crate::cli::CmdError;

use super::super::{Action, ActionKind, Rollback};
use super::locator::valid_component;

pub(super) fn validate_rollback(action: &Action, rollback: &Rollback) -> Result<(), CmdError> {
    let valid = match (action.kind, rollback.kind) {
        (ActionKind::SnapshotDisk, ActionKind::DeleteSnapshot) => {
            let planned = action
                .parameters
                .get("snapshot_name")
                .and_then(Value::as_str);
            let restored = rollback
                .parameters
                .get("snapshot_name")
                .and_then(Value::as_str);
            planned == restored && planned.is_some_and(valid_component)
        }
        (ActionKind::DeleteDisk, ActionKind::RestoreDisk) => {
            let planned = action
                .parameters
                .get("snapshot_name")
                .and_then(Value::as_str);
            let restored = rollback
                .parameters
                .get("snapshot_name")
                .and_then(Value::as_str);
            planned == restored
                && planned.is_some_and(valid_component)
                && action.parameters.get("scope") == rollback.parameters.get("scope")
                && action
                    .parameters
                    .get("original")
                    .is_some_and(Value::is_object)
                && action.parameters.get("original") == rollback.parameters.get("original")
        }
        (ActionKind::DisableStorageBackup, ActionKind::EnableStorageBackup) => rollback
            .parameters
            .get("backup")
            .is_some_and(|value| value.is_object()),
        (ActionKind::PauseScheduler, ActionKind::ResumeScheduler)
        | (ActionKind::StopInstance, ActionKind::StartInstance)
        | (ActionKind::StartInstance, ActionKind::StopInstance) => rollback
            .parameters
            .as_object()
            .is_some_and(serde_json::Map::is_empty),
        (ActionKind::ResizeManagedInstanceGroup, ActionKind::ResizeManagedInstanceGroup) => {
            rollback
                .parameters
                .get("target_size")
                .and_then(Value::as_i64)
                .is_some_and(|size| size >= i64::default())
                && rollback.parameters.get("scope") == action.parameters.get("scope")
        }
        (ActionKind::SuspendCloudSql, ActionKind::RestoreCloudSql) => rollback
            .parameters
            .get("activation_policy")
            .and_then(Value::as_str)
            .is_some_and(|policy| {
                !policy.is_empty()
                    && policy.len() <= u8::MAX as usize
                    && policy
                        .bytes()
                        .all(|byte| byte.is_ascii_uppercase() || byte == b'_')
            }),
        _ => false,
    };
    if !valid {
        return Err(CmdError::click(format!(
            "action {} has unsafe rollback parameters",
            action.id
        )));
    }
    Ok(())
}

pub(super) fn rollback_pair(action: ActionKind, rollback: ActionKind) -> bool {
    matches!(
        (action, rollback),
        (ActionKind::SnapshotDisk, ActionKind::DeleteSnapshot)
            | (ActionKind::DeleteDisk, ActionKind::RestoreDisk)
            | (
                ActionKind::DisableStorageBackup,
                ActionKind::EnableStorageBackup
            )
            | (ActionKind::PauseScheduler, ActionKind::ResumeScheduler)
            | (
                ActionKind::ResizeManagedInstanceGroup,
                ActionKind::ResizeManagedInstanceGroup
            )
            | (ActionKind::StopInstance, ActionKind::StartInstance)
            | (ActionKind::StartInstance, ActionKind::StopInstance)
            | (ActionKind::SuspendCloudSql, ActionKind::RestoreCloudSql)
    )
}
