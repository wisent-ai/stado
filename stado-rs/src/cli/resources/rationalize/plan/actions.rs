//! One resource family per arm: the actions a finding authorises, how far
//! each one can be undone, and the scope the executor addresses it with. A
//! family with no safe action contributes a finding and no action at all.

use serde_json::{json, Value};

use crate::cli::resources::model::{Action, ActionKind, Authorization, Reversibility, Rollback};
use crate::cli::resources::planner;
use crate::cli::resources::rationalize::Finding;

use super::conditions::{
    disk_restore_postconditions, irreversible_action, locator, recovery_snapshot_name,
    stable_preconditions,
};

pub(super) fn actions_for(finding: &Finding) -> Vec<Action> {
    let resource_scope = gcp_scope(finding);
    let resource = locator(finding);
    let authorization = if finding.automatic {
        Authorization::Automatic
    } else {
        Authorization::Explicit
    };
    let action_id = format!("action-{}", uuid::Uuid::new_v4().simple());
    match finding.resource_type {
        "agent-vm" => vec![Action {
            id: action_id,
            finding_id: Some(finding.id.clone()),
            kind: ActionKind::DeleteInstance,
            authorization,
            reversibility: Reversibility::Irreversible,
            resource,
            parameters: json!({}),
            preconditions: vec![
                planner::condition("orphan", json!(true)),
                planner::condition(
                    "minimum_age_seconds",
                    finding.evidence["age_seconds"].clone(),
                ),
            ],
            postconditions: vec![planner::condition("exists", json!(false))],
            rollback: None,
            depends_on: Vec::new(),
        }],
        "persistent-disk" => {
            let snapshot_id = format!("action-{}", uuid::Uuid::new_v4().simple());
            let snapshot_name = recovery_snapshot_name(&resource.name);
            vec![
                Action {
                    id: snapshot_id.clone(),
                    finding_id: Some(finding.id.clone()),
                    kind: ActionKind::SnapshotDisk,
                    authorization: Authorization::Explicit,
                    reversibility: Reversibility::Reversible,
                    resource: resource.clone(),
                    parameters: json!({"snapshot_name": snapshot_name, "scope": resource_scope}),
                    preconditions: stable_preconditions(
                        finding,
                        vec![planner::condition("unattached", json!(true))],
                    ),
                    postconditions: vec![planner::condition("snapshot_exists", json!(true))],
                    rollback: Some(Rollback {
                        kind: ActionKind::DeleteSnapshot,
                        parameters: json!({"snapshot_name": snapshot_name}),
                        preconditions: vec![planner::condition("snapshot_exists", json!(true))],
                        postconditions: vec![planner::condition("snapshot_exists", json!(false))],
                    }),
                    depends_on: Vec::new(),
                },
                Action {
                    id: action_id,
                    finding_id: Some(finding.id.clone()),
                    kind: ActionKind::DeleteDisk,
                    authorization: Authorization::Explicit,
                    reversibility: Reversibility::SnapshotRestore,
                    resource,
                    parameters: json!({"snapshot_name": snapshot_name, "scope": resource_scope, "original": finding.evidence}),
                    preconditions: stable_preconditions(
                        finding,
                        vec![planner::condition("unattached", json!(true))],
                    ),
                    postconditions: vec![planner::condition("exists", json!(false))],
                    rollback: Some(Rollback {
                        kind: ActionKind::RestoreDisk,
                        parameters: json!({"snapshot_name": snapshot_name, "scope": resource_scope, "original": finding.evidence}),
                        preconditions: vec![
                            planner::condition("exists", json!(false)),
                            planner::condition("snapshot_exists", json!(true)),
                        ],
                        postconditions: disk_restore_postconditions(finding, &snapshot_name),
                    }),
                    depends_on: vec![snapshot_id],
                },
            ]
        }
        "static-address" => vec![irreversible_action(
            action_id,
            finding,
            ActionKind::ReleaseAddress,
            resource,
            json!({"scope": resource_scope}),
            stable_preconditions(
                finding,
                vec![
                    planner::condition("unused", json!(true)),
                    planner::condition("status", json!("RESERVED")),
                ],
            ),
        )],
        "managed-instance-group" => vec![irreversible_action(
            action_id,
            finding,
            ActionKind::DeleteManagedInstanceGroup,
            resource,
            json!({"scope": resource_scope}),
            stable_preconditions(
                finding,
                vec![planner::condition("target_size", json!(usize::default()))],
            ),
        )],
        "compute-reservation" => vec![irreversible_action(
            action_id,
            finding,
            ActionKind::ReleaseReservation,
            resource,
            json!({"scope": resource_scope}),
            stable_preconditions(
                finding,
                vec![
                    planner::condition("exists", json!(true)),
                    planner::condition("in_use_count", json!(usize::default())),
                ],
            ),
        )],
        "storage-backup"
            if finding.action == "disable" && finding.evidence["backup_config"].is_object() =>
        {
            vec![Action {
                id: action_id,
                finding_id: Some(finding.id.clone()),
                kind: ActionKind::DisableStorageBackup,
                authorization: Authorization::Explicit,
                reversibility: Reversibility::Reversible,
                resource,
                parameters: json!({"previous": finding.evidence["backup_config"]}),
                preconditions: vec![
                    planner::condition("configured", json!(true)),
                    planner::condition("mutable", json!(true)),
                    planner::condition("backup", finding.evidence["backup_config"].clone()),
                ],
                postconditions: vec![planner::condition("configured", json!(false))],
                rollback: Some(Rollback {
                    kind: ActionKind::EnableStorageBackup,
                    parameters: json!({"backup": finding.evidence["backup_config"]}),
                    preconditions: vec![planner::condition("configured", json!(false))],
                    postconditions: vec![
                        planner::condition("configured", json!(true)),
                        planner::condition("backup", finding.evidence["backup_config"].clone()),
                    ],
                }),
                depends_on: Vec::new(),
            }]
        }
        _ => Vec::new(),
    }
}
fn gcp_scope(finding: &Finding) -> &'static str {
    let region = finding
        .evidence
        .get("region")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if finding.resource_type == "static-address" && region.is_empty() {
        "global"
    } else if !region.is_empty() {
        "region"
    } else {
        "zone"
    }
}
