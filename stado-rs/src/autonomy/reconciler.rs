//! Continuous resource reconciliation using the existing immutable resource-plan engine.

use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::cli::resources::model::{
    Action, ActionKind, Authorization, Condition, Finding, FindingDisposition, Intent,
    InventorySnapshot as OperationInventorySnapshot, OperationScope, Plan, ResourceLocator,
    Reversibility, Rollback, SourceSnapshot,
};
use crate::queue::{JobStorage, StorageError};

use super::model::{InventorySnapshot, Ownership, ResourceRecord};
use super::policy::{AutonomyMode, AutonomyPolicy};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ReconcileSummary {
    pub operation_id: Option<String>,
    pub findings: usize,
    pub automatic_actions: usize,
    pub scheduled_actions: usize,
    pub executed: bool,
    pub blocked_reason: Option<String>,
}

pub async fn reconcile(
    store: &JobStorage,
    snapshot: &InventorySnapshot,
    policy: &AutonomyPolicy,
    configuration_fingerprint: &str,
    log: &dyn Fn(&str),
) -> Result<ReconcileSummary, StorageError> {
    let mut summary = ReconcileSummary::default();
    if policy.emergency_paused {
        summary.blocked_reason = Some("emergency pause is active".to_string());
        return Ok(summary);
    }
    if !snapshot.complete {
        summary.blocked_reason = Some("inventory is incomplete".to_string());
        return Ok(summary);
    }
    if policy.mode != AutonomyMode::Report {
        summary.scheduled_actions =
            reconcile_schedules(store, snapshot, policy, configuration_fingerprint).await?;
        if summary.scheduled_actions > usize::default() {
            summary.automatic_actions = summary.scheduled_actions;
            summary.executed = true;
            return Ok(summary);
        }
    }
    let plan = build_plan(snapshot, policy, configuration_fingerprint)?;
    summary.operation_id = Some(plan.operation_id.clone());
    summary.findings = plan.findings.len();
    summary.automatic_actions = plan.actions.len();
    super::storage::write_json(
        store,
        &format!("state/autonomy/plans/{}.json", plan.operation_id),
        &plan,
        false,
    )
    .await?;
    if policy.mode == AutonomyMode::Report || plan.actions.is_empty() {
        return Ok(summary);
    }
    log(&format!(
        "autonomy: executing {} bounded owned-resource action(s) from plan {}",
        plan.actions.len(),
        plan.operation_id
    ));
    execute_with_circuit(store, &plan, policy).await?;
    summary.executed = true;
    Ok(summary)
}
async fn execute_with_circuit(
    store: &JobStorage,
    plan: &Plan,
    policy: &AutonomyPolicy,
) -> Result<(), StorageError> {
    let mut mutation_lease = None;
    for slot in usize::default()..policy.limits.max_concurrent_mutations {
        let subject = format!("mutation-slot-{slot}");
        if let Some(lease) = super::storage::acquire_placement_lease(
            store,
            &subject,
            &plan.operation_id,
            "autonomy-reconciler",
            policy.limits.decision_ttl_seconds,
            Utc::now(),
        )
        .await?
        {
            mutation_lease = Some((subject, lease.token));
            break;
        }
    }
    let Some((lease_subject, lease_token)) = mutation_lease else {
        return Err(StorageError::Other(
            "autonomy mutation concurrency limit reached".to_string(),
        ));
    };

    let control_gate = match super::storage::load_control(store).await {
        Ok(control) if control.emergency_paused => {
            Some("autonomy emergency pause became active before mutation".to_string())
        }
        Ok(control) if control.circuit_open_at(Utc::now()) => Some(format!(
            "autonomy circuit breaker opened before mutation until {}",
            control.circuit_open_until.as_deref().unwrap_or("unknown")
        )),
        Ok(_) => None,
        Err(error) => Some(format!(
            "autonomy control state became unreadable before mutation: {error}"
        )),
    };
    if let Some(mut detail) = control_gate {
        if let Some(release_error) =
            release_mutation_lease(store, &lease_subject, &lease_token).await
        {
            detail.push_str("; ");
            detail.push_str(&release_error);
        }
        return Err(StorageError::Other(detail));
    }

    let execution = crate::cli::resources::engine::execute_autonomous(plan).await;
    let release_error = release_mutation_lease(store, &lease_subject, &lease_token).await;
    match execution {
        Ok(()) => {
            super::storage::record_mutation_outcome(
                store,
                true,
                None,
                policy.limits.circuit_breaker_failures,
                policy.limits.circuit_breaker_cooldown_seconds,
            )
            .await?;
            if let Some(error) = release_error {
                return Err(StorageError::Other(error));
            }
            Ok(())
        }
        Err(error) => {
            let mut detail = error.to_string();
            if let Some(release_error) = release_error {
                detail.push_str("; ");
                detail.push_str(&release_error);
            }
            super::storage::record_mutation_outcome(
                store,
                false,
                Some(&detail),
                policy.limits.circuit_breaker_failures,
                policy.limits.circuit_breaker_cooldown_seconds,
            )
            .await?;
            Err(StorageError::Other(detail))
        }
    }
}
async fn release_mutation_lease(store: &JobStorage, subject: &str, token: &str) -> Option<String> {
    match super::storage::release_placement_lease(store, subject, token).await {
        Ok(true) => None,
        Ok(false) => Some("mutation lease ownership changed before release".to_string()),
        Err(error) => Some(format!("mutation lease release failed: {error}")),
    }
}

async fn reconcile_schedules(
    store: &JobStorage,
    snapshot: &InventorySnapshot,
    policy: &AutonomyPolicy,
    configuration_fingerprint: &str,
) -> Result<usize, StorageError> {
    let now = Utc::now();
    let lookback = chrono::Duration::seconds(policy.limits.decision_ttl_seconds as i64);
    let mut executed = usize::default();
    let mut provider_actions: BTreeMap<crate::capabilities::ProviderId, usize> = BTreeMap::new();
    for resource in &snapshot.resources {
        if executed >= policy.limits.max_actions_per_tick {
            break;
        }
        if provider_actions
            .get(&resource.provider)
            .copied()
            .unwrap_or_default()
            >= policy.limits.max_actions_per_provider
        {
            continue;
        }
        if resource.resource_type != "instance"
            || resource.workload.is_some()
            || !matches!(resource.ownership, Ownership::Owned | Ownership::Adopted)
        {
            continue;
        }
        let Some(rule) = policy.matching_rule(resource) else {
            continue;
        };
        let authorization = policy.authorize(
            resource,
            super::policy::ActionRisk::Reversible,
            snapshot.complete,
            resource.current_hourly_cost_usd,
        );
        if !authorization.allowed {
            continue;
        }
        let state = resource.state.to_ascii_lowercase();
        let running = matches!(
            state.as_str(),
            "running" | "staging" | "provisioning" | "pending" | "succeeded"
        );
        let stopped = matches!(
            state.as_str(),
            "stopped" | "stopping" | "deallocated" | "deallocating"
        ) || (resource.provider == crate::capabilities::ProviderId::Gcp
            && state == "terminated");
        let scheduled = if running {
            rule.stop_schedule
                .as_deref()
                .map(|expression| (ActionKind::StopInstance, expression))
        } else if stopped {
            rule.start_schedule
                .as_deref()
                .map(|expression| (ActionKind::StartInstance, expression))
        } else {
            None
        };
        let Some((kind, expression)) = scheduled else {
            continue;
        };
        let timezone = rule.timezone.as_deref().unwrap_or("UTC");
        let occurrence = crate::schedules::compute_next_due(expression, now - lookback, timezone)
            .map_err(|error| StorageError::Other(error.to_string()))?;
        if occurrence > now {
            continue;
        }
        let marker_key = format!(
            "{}:{kind:?}:{}",
            resource.resource_id,
            occurrence.to_rfc3339()
        );
        let marker_hash = hex::encode(Sha256::digest(marker_key.as_bytes()));
        let marker_path = format!("state/autonomy/decisions/schedule-{marker_hash}.json");
        if !acquire_schedule_marker(store, &marker_path, policy, now).await? {
            continue;
        }
        let plan = schedule_plan(
            snapshot,
            resource,
            rule,
            kind,
            occurrence,
            configuration_fingerprint,
            policy,
        )?;
        super::storage::write_json(
            store,
            &format!("state/autonomy/plans/{}.json", plan.operation_id),
            &plan,
            false,
        )
        .await?;
        if let Err(error) = execute_with_circuit(store, &plan, policy).await {
            let _ = store.delete_blob(&marker_path).await;
            return Err(error);
        }
        store
            .upload_text(
                &marker_path,
                &serde_json::to_string(&json!({
                    "status": "completed",
                    "completed_at": Utc::now().to_rfc3339(),
                    "operation_id": plan.operation_id,
                    "occurrence": occurrence.to_rfc3339(),
                }))?,
            )
            .await?;
        *provider_actions.entry(resource.provider).or_default() += true as usize;
        executed += true as usize;
    }
    Ok(executed)
}

async fn acquire_schedule_marker(
    store: &JobStorage,
    path: &str,
    policy: &AutonomyPolicy,
    now: DateTime<Utc>,
) -> Result<bool, StorageError> {
    let content = serde_json::to_string(&json!({
        "status": "in_progress",
        "started_at": now.to_rfc3339(),
    }))?;
    if store.create_text_if_absent(path, &content).await? {
        return Ok(true);
    }
    let Some(existing) = store.read_text_versioned(path).await? else {
        return Ok(false);
    };
    let value: Value = serde_json::from_str(&existing.content)?;
    if value.get("status").and_then(Value::as_str) == Some("completed") {
        return Ok(false);
    }
    let fresh = value
        .get("started_at")
        .and_then(Value::as_str)
        .and_then(|raw| DateTime::parse_from_rfc3339(raw).ok())
        .is_some_and(|started| {
            now.signed_duration_since(started.with_timezone(&Utc))
                .num_seconds()
                < policy.limits.decision_ttl_seconds as i64
        });
    if fresh {
        return Ok(false);
    }
    store
        .compare_and_swap_text(path, &existing.version, &content)
        .await?;
    Ok(true)
}

fn schedule_plan(
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

struct Classification {
    severity: &'static str,
    confidence: &'static str,
    recommendation: &'static str,
    reason: &'static str,
    disposition: FindingDisposition,
    action: Option<ActionKind>,
}

fn classify(
    resource: &ResourceRecord,
    age_seconds: u64,
    policy: &AutonomyPolicy,
) -> Option<Classification> {
    let active = !matches!(
        resource.state.to_ascii_lowercase().as_str(),
        "terminated" | "deleted" | "deleting" | "failed"
    );
    if !active {
        return None;
    }
    let unowned = matches!(resource.ownership, Ownership::Observed | Ownership::Unknown);
    if resource.resource_type == "instance"
        && resource.workload.is_none()
        && age_seconds >= policy.idle.vm_seconds
        && low_utilization(resource)
    {
        if unowned {
            return Some(Classification {
                severity: "medium",
                confidence: "high",
                recommendation: "adopt explicitly or terminate manually",
                reason: "idle instance is not owned by Stado",
                disposition: FindingDisposition::Blocked,
                action: None,
            });
        }
        let scale_to_zero = policy
            .matching_rule(resource)
            .is_some_and(|rule| rule.scale_to_zero);
        if scale_to_zero {
            let running = matches!(
                resource.state.to_ascii_lowercase().as_str(),
                "running" | "staging" | "provisioning" | "pending"
            );
            if !running {
                return None;
            }
            return Some(Classification {
                severity: "medium",
                confidence: "high",
                recommendation: "stop idle instance according to scale-to-zero policy",
                reason: "owned/adopted instance is idle and exceeded the scale-to-zero TTL",
                disposition: if policy.mode == AutonomyMode::Report {
                    FindingDisposition::ReviewRequired
                } else {
                    FindingDisposition::Automatic
                },
                action: Some(ActionKind::StopInstance),
            });
        }
        return Some(Classification {
            severity: "high",
            confidence: "high",
            recommendation: "terminate idle Stado instance",
            reason: "owned/adopted instance has no workload and exceeded the idle TTL",
            disposition: if policy.mode == AutonomyMode::EnforceOwned {
                FindingDisposition::Automatic
            } else {
                FindingDisposition::ReviewRequired
            },
            action: Some(ActionKind::DeleteInstance),
        });
    }
    let (threshold, recommendation, reason) = match resource.resource_type.as_str() {
        "persistent_disk" | "managed_disk" | "volume" => (
            policy.idle.disk_days * crate::monitor::billing::SECONDS_PER_DAY,
            "snapshot if needed, then delete orphaned disk",
            "unattached storage exceeded the idle TTL",
        ),
        "snapshot" => (
            policy.idle.snapshot_days * crate::monitor::billing::SECONDS_PER_DAY,
            "expire snapshot according to retention policy",
            "snapshot exceeded the retention TTL",
        ),
        "public_ip" | "static_address" => (
            policy.idle.disk_days * crate::monitor::billing::SECONDS_PER_DAY,
            "release unused address",
            "unattached address exceeded the idle TTL",
        ),
        "image" | "artifact_repository" | "bucket" => (
            policy.idle.artifact_days * crate::monitor::billing::SECONDS_PER_DAY,
            "apply artifact lifecycle policy after dependency review",
            "artifact exceeded the retention TTL",
        ),
        _ => return None,
    };
    if resource.workload.is_some() || age_seconds < threshold {
        return None;
    }
    Some(Classification {
        severity: "medium",
        confidence: if resource.dependencies.is_empty() {
            "medium"
        } else {
            "low"
        },
        recommendation,
        reason,
        disposition: if unowned {
            FindingDisposition::Blocked
        } else {
            FindingDisposition::ReviewRequired
        },
        action: None,
    })
}

fn low_utilization(resource: &ResourceRecord) -> bool {
    let state = resource.state.to_ascii_lowercase();
    let stopped = matches!(
        state.as_str(),
        "stopped" | "stopping" | "deallocated" | "deallocating" | "terminated"
    );
    stopped
        || (!resource.utilization.is_empty()
            && resource
                .utilization
                .values()
                .all(|value| *value <= f64::EPSILON))
}

fn resource_age_seconds(resource: &ResourceRecord, now: DateTime<Utc>) -> Option<u64> {
    let raw = resource.created_at.as_deref()?;
    let created = DateTime::parse_from_rfc3339(raw).ok()?.with_timezone(&Utc);
    now.signed_duration_since(created)
        .num_seconds()
        .try_into()
        .ok()
}

fn locator(resource: &ResourceRecord) -> ResourceLocator {
    ResourceLocator {
        provider: resource.provider,
        resource_type: if resource.resource_type == "instance" {
            "agent-vm".to_string()
        } else {
            resource.resource_type.clone()
        },
        project: (resource.provider == crate::capabilities::ProviderId::Gcp)
            .then(|| resource.account.clone()),
        location: resource.zone.clone().or_else(|| resource.region.clone()),
        name: resource.name.clone(),
        reference: execution_reference(resource),
    }
}

fn execution_reference(resource: &ResourceRecord) -> String {
    match resource.provider {
        crate::capabilities::ProviderId::Gcp => resource
            .zone
            .as_deref()
            .map(tail)
            .map(|zone| format!("{}@{zone}", resource.name))
            .unwrap_or_else(|| resource.native_reference.clone()),
        crate::capabilities::ProviderId::Azure => resource
            .region
            .as_deref()
            .map(|region| format!("{}@{region}", resource.name))
            .unwrap_or_else(|| resource.native_reference.clone()),
        _ => resource.native_reference.clone(),
    }
}

fn tail(value: &str) -> &str {
    value.rsplit('/').next().unwrap_or(value)
}

fn ownership_name(ownership: Ownership) -> &'static str {
    match ownership {
        Ownership::Owned => "owned",
        Ownership::Adopted => "adopted",
        Ownership::Observed => "observed",
        Ownership::Unknown => "unknown",
    }
}
