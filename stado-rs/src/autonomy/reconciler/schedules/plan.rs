//! The one-action plan a due start or stop expression turns into, with the
//! reverse action already attached as its rollback.

use std::collections::BTreeSet;

use chrono::{DateTime, Utc};
use serde_json::{json, Value};

use crate::cli::resources::model::{
    Action, ActionKind, Authorization, Condition, Finding, FindingDisposition, Intent,
    InventorySnapshot as OperationInventorySnapshot, OperationScope, Plan, Reversibility, Rollback,
    SourceSnapshot,
};
use crate::queue::StorageError;

use crate::autonomy::model::{InventorySnapshot, ResourceRecord};
use crate::autonomy::reconciler::resource::{locator, ownership_name};

use super::policy::AutonomyPolicy;

pub(super) fn schedule_plan(
    snapshot: &InventorySnapshot,
    resource: &ResourceRecord,
    rule: &super::policy::ResourceRule,
    kind: ActionKind,
    occurrence: DateTime<Utc>,
    configuration_fingerprint: &str,
    policy: &AutonomyPolicy,
) -> Result<Plan, StorageError> {
    let created = Utc::now();
    let expires = created + chrono::Duration::seconds(policy.limits.decision_ttl_seconds as i64);
    let operation_id = format!("autonomy-schedule-{}", uuid::Uuid::new_v4());
    let action_id = format!("action-{}", uuid::Uuid::new_v4());
    let finding_id = format!("finding-{}", uuid::Uuid::new_v4());
    let resource_locator = locator(resource);
    let (preconditions, postconditions, rollback_kind, rollback_pre, rollback_post) = match kind {
        ActionKind::StopInstance => (
            vec![
                Condition {
                    field: "running".to_string(),
                    expected: Value::Bool(true),
                },
                Condition {
                    field: "orphan".to_string(),
                    expected: Value::Bool(true),
                },
            ],
            vec![Condition {
                field: "stopped".to_string(),
                expected: Value::Bool(true),
            }],
            ActionKind::StartInstance,
            vec![Condition {
                field: "stopped".to_string(),
                expected: Value::Bool(true),
            }],
            vec![Condition {
                field: "running".to_string(),
                expected: Value::Bool(true),
            }],
        ),
        ActionKind::StartInstance => (
            vec![Condition {
                field: "stopped".to_string(),
                expected: Value::Bool(true),
            }],
            vec![Condition {
                field: "running".to_string(),
                expected: Value::Bool(true),
            }],
            ActionKind::StopInstance,
            vec![Condition {
                field: "running".to_string(),
                expected: Value::Bool(true),
            }],
            vec![Condition {
                field: "stopped".to_string(),
                expected: Value::Bool(true),
            }],
        ),
        _ => {
            return Err(StorageError::Other(
                "schedule plan only supports start/stop".to_string(),
            ))
        }
    };
    let finding = Finding {
        id: finding_id.clone(),
        severity: "medium".to_string(),
        confidence: "high".to_string(),
        recommendation: format!("{kind:?} according to resource schedule"),
        reason: format!(
            "resource rule {} occurrence {} is due",
            rule.policy_ref,
            occurrence.to_rfc3339()
        ),
        evidence: json!({
            "resource_id": resource.resource_id,
            "state": resource.state,
            "occurrence": occurrence.to_rfc3339(),
            "policy_ref": rule.policy_ref,
        }),
        disposition: FindingDisposition::Automatic,
        resource: resource_locator.clone(),
    };
    let action = Action {
        id: action_id,
        finding_id: Some(finding_id),
        kind,
        authorization: Authorization::Automatic,
        reversibility: Reversibility::Reversible,
        resource: resource_locator,
        parameters: json!({
            "resource_id": resource.resource_id,
            "resource_revision": resource.source_revision,
            "ownership": ownership_name(resource.ownership),
            "policy_ref": rule.policy_ref,
            "occurrence": occurrence.to_rfc3339(),
        }),
        preconditions,
        postconditions,
        rollback: Some(Rollback {
            kind: rollback_kind,
            parameters: json!({}),
            preconditions: rollback_pre,
            postconditions: rollback_post,
        }),
        depends_on: Vec::new(),
    };
    let mut providers = BTreeSet::new();
    providers.insert(resource.provider);
    let mut projects = BTreeSet::new();
    if resource.provider == crate::capabilities::ProviderId::Gcp {
        projects.insert(resource.account.clone());
    }
    let plan = Plan {
        schema_version: crate::cli::resources::model::SCHEMA_VERSION,
        operation_id,
        intent: Intent::AutonomousReconcile,
        created_at: created.to_rfc3339(),
        expires_at: expires.to_rfc3339(),
        stado_version: env!("CARGO_PKG_VERSION").to_string(),
        scope: OperationScope {
            providers,
            projects,
            storage: "canonical-job-storage".to_string(),
        },
        configuration_fingerprint: configuration_fingerprint.to_string(),
        inventory: OperationInventorySnapshot {
            snapshot_id: snapshot.snapshot_id.clone(),
            complete: snapshot.complete,
            sources: snapshot
                .sources
                .iter()
                .map(|source| SourceSnapshot {
                    name: format!("{}:{}", source.provider.as_str(), source.account),
                    state: format!("{:?}", source.state).to_ascii_lowercase(),
                    detail: json!({
                        "coverage": source.coverage,
                        "missing_permissions": source.missing_permissions,
                        "upstream_error": source.upstream_error,
                    }),
                })
                .collect(),
        },
        findings: vec![finding],
        actions: vec![action],
    };
    plan.validate()
        .map_err(|error| StorageError::Other(error.to_string()))?;
    Ok(plan)
}
