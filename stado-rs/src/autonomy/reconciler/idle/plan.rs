//! The drift plan: one finding per classified resource and, within the
//! per-tick and per-provider bounds, the authorized action it carries.

use std::collections::{BTreeMap, BTreeSet};

use chrono::Utc;
use serde_json::{json, Value};

use crate::cli::resources::model::{
    Action, ActionKind, Authorization, Condition, Finding, FindingDisposition, Intent,
    InventorySnapshot as OperationInventorySnapshot, OperationScope, Plan, Reversibility, Rollback,
    SourceSnapshot,
};
use crate::queue::StorageError;

use crate::autonomy::model::{InventorySnapshot, Ownership};
use crate::autonomy::reconciler::resource::{locator, ownership_name, resource_age_seconds};

use super::classify;
use super::policy::{AutonomyMode, AutonomyPolicy};

pub fn build_plan(
    snapshot: &InventorySnapshot,
    policy: &AutonomyPolicy,
    configuration_fingerprint: &str,
) -> Result<Plan, StorageError> {
    let created = Utc::now();
    let expires = created + chrono::Duration::seconds(policy.limits.decision_ttl_seconds as i64);
    let operation_id = format!("autonomy-{}", uuid::Uuid::new_v4());
    let mut findings = Vec::new();
    let mut actions = Vec::new();
    let mut providers = BTreeSet::new();
    let mut projects = BTreeSet::new();
    let mut provider_action_counts: BTreeMap<crate::capabilities::ProviderId, usize> =
        BTreeMap::new();

    for source in &snapshot.sources {
        providers.insert(source.provider);
        if source.provider == crate::capabilities::ProviderId::Gcp {
            projects.insert(source.account.clone());
        }
        for resource in &source.resources {
            let Some(age_seconds) = resource_age_seconds(resource, created) else {
                continue;
            };
            let Some(classification) = classify(resource, age_seconds, policy) else {
                continue;
            };
            let locator = locator(resource);
            let finding_id = format!("finding-{}", uuid::Uuid::new_v4());
            let action_risk = match classification.action {
                Some(ActionKind::StopInstance) | Some(ActionKind::StartInstance) => {
                    super::policy::ActionRisk::Reversible
                }
                _ => super::policy::ActionRisk::Destructive,
            };
            let authorization = policy.authorize(resource, action_risk, snapshot.complete, None);
            let finding = Finding {
                id: finding_id.clone(),
                severity: classification.severity.to_string(),
                confidence: classification.confidence.to_string(),
                recommendation: classification.recommendation.to_string(),
                reason: classification.reason.to_string(),
                evidence: json!({
                    "resource_id": resource.resource_id,
                    "ownership": resource.ownership,
                    "age_seconds": age_seconds,
                    "state": resource.state,
                    "workload": resource.workload,
                    "utilization": resource.utilization,
                    "current_hourly_cost_usd": resource.current_hourly_cost_usd,
                    "source_revision": resource.source_revision,
                }),
                disposition: if classification.action.is_some() && !authorization.allowed {
                    FindingDisposition::Blocked
                } else {
                    classification.disposition
                },
                resource: locator.clone(),
            };
            let can_act = classification.action.is_some()
                && policy.mode != AutonomyMode::Report
                && matches!(resource.ownership, Ownership::Owned | Ownership::Adopted)
                && authorization.allowed
                && actions.len() < policy.limits.max_actions_per_tick
                && provider_action_counts
                    .get(&resource.provider)
                    .copied()
                    .unwrap_or_default()
                    < policy.limits.max_actions_per_provider;
            findings.push(finding);
            if !can_act {
                continue;
            }
            let action_id = format!("action-{}", uuid::Uuid::new_v4());
            let kind = classification
                .action
                .expect("action-bearing classification checked above");
            let (reversibility, postconditions, rollback) = match kind {
                ActionKind::StopInstance => (
                    Reversibility::Reversible,
                    vec![Condition {
                        field: "stopped".to_string(),
                        expected: Value::Bool(true),
                    }],
                    Some(Rollback {
                        kind: ActionKind::StartInstance,
                        parameters: json!({}),
                        preconditions: vec![Condition {
                            field: "stopped".to_string(),
                            expected: Value::Bool(true),
                        }],
                        postconditions: vec![Condition {
                            field: "running".to_string(),
                            expected: Value::Bool(true),
                        }],
                    }),
                ),
                ActionKind::DeleteInstance => (
                    Reversibility::Irreversible,
                    vec![Condition {
                        field: "exists".to_string(),
                        expected: Value::Bool(false),
                    }],
                    None,
                ),
                _ => continue,
            };
            actions.push(Action {
                id: action_id,
                finding_id: Some(finding_id),
                kind,
                authorization: Authorization::Automatic,
                reversibility,
                resource: locator,
                parameters: json!({
                    "ownership": ownership_name(resource.ownership),
                    "minimum_age_seconds": policy.idle.vm_seconds,
                    "resource_id": resource.resource_id,
                    "resource_revision": resource.source_revision,
                    "inventory_snapshot_id": snapshot.snapshot_id,
                }),
                preconditions: vec![
                    Condition {
                        field: "exists".to_string(),
                        expected: Value::Bool(true),
                    },
                    Condition {
                        field: "orphan".to_string(),
                        expected: Value::Bool(true),
                    },
                    Condition {
                        field: "minimum_age_seconds".to_string(),
                        expected: Value::from(policy.idle.vm_seconds),
                    },
                ],
                postconditions,
                rollback,
                depends_on: Vec::new(),
            });
            *provider_action_counts.entry(resource.provider).or_default() += true as usize;
        }
    }

    if actions.len() > policy.limits.max_actions_per_tick {
        return Err(StorageError::Other(
            "autonomy plan exceeds max_actions_per_tick".to_string(),
        ));
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
        findings,
        actions,
    };
    plan.validate()
        .map_err(|error| StorageError::Other(error.to_string()))?;
    Ok(plan)
}
