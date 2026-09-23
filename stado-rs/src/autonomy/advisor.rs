//! Rightsizing, scheduling, storage-lifecycle, and commitment recommendations.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::queue::{JobStorage, StorageError};

use super::model::{
    DecisionKind, DecisionRecord, InventorySnapshot, ResourceRecord, SCHEMA_VERSION,
};
use super::policy::{ActionRisk, AutonomyPolicy};

const TWO: u8 = (u16::BITS / u8::BITS) as u8;
const QUARTER: f64 = (true as u8) as f64 / (TWO * TWO) as f64;
const PERCENT: f64 = ((u8::BITS as u8 + TWO) * (u8::BITS as u8 + TWO)) as f64;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdvisorSummary {
    pub rightsizing: usize,
    pub schedules: usize,
    pub storage_lifecycle: usize,
    pub network: usize,
    pub commitments: usize,
}

struct RecommendationContext<'a> {
    snapshot: &'a InventorySnapshot,
    policy: &'a AutonomyPolicy,
    now: DateTime<Utc>,
}

pub async fn publish_recommendations(
    store: &JobStorage,
    snapshot: &InventorySnapshot,
    policy: &AutonomyPolicy,
    now: DateTime<Utc>,
) -> Result<AdvisorSummary, StorageError> {
    let mut summary = AdvisorSummary::default();
    let context = RecommendationContext {
        snapshot,
        policy,
        now,
    };
    for resource in &snapshot.resources {
        if underutilized(resource)
            && resource.current_hourly_cost_usd.is_some()
            && publish(
                store,
                context.recommendation(
                    resource,
                    DecisionKind::Rightsize,
                    json!({
                        "operation": "replace_on_next_idle_cycle",
                        "target": "smallest provider shape satisfying observed peak plus headroom",
                        "cpu_peak": utilization(resource, &["cpu_peak", "cpu", "cpu_max"]),
                        "memory_peak": utilization(resource, &["memory_peak", "memory", "memory_max"]),
                        "gpu_peak": utilization(resource, &["gpu_peak", "gpu", "gpu_max"]),
                    }),
                    "Resource stayed below the rightsizing utilization threshold; replace only after it is idle",
                    ActionRisk::Reversible,
                ),
            )
            .await?
            {
                summary.rightsizing += true as usize;
            }
        if let Some(rule) = policy.matching_rule(resource) {
            if (rule.stop_schedule.is_some() || rule.start_schedule.is_some() || rule.scale_to_zero)
                && publish(
                    store,
                    context.recommendation(
                        resource,
                        DecisionKind::Schedule,
                        json!({
                            "stop_schedule": rule.stop_schedule,
                            "start_schedule": rule.start_schedule,
                            "timezone": rule.timezone,
                            "scale_to_zero": rule.scale_to_zero,
                            "policy_ref": rule.policy_ref,
                        }),
                        "A versioned resource rule defines a start/stop or scale-to-zero policy",
                        ActionRisk::Reversible,
                    ),
                )
                .await?
            {
                summary.schedules += true as usize;
            }
        }
        if storage_candidate(resource, policy, now)
            && publish(
                store,
                context.recommendation(
                    resource,
                    DecisionKind::StorageLifecycle,
                    json!({
                        "operation": "snapshot_then_expire_or_transition",
                        "minimum_snapshots": policy.idle.minimum_snapshots,
                        "retention_days": policy.idle.disk_days,
                    }),
                    "Unattached storage exceeded the configured lifecycle age",
                    ActionRisk::Destructive,
                ),
            )
            .await?
        {
            summary.storage_lifecycle += true as usize;
        }
        if policy.placement.account_for_egress {
            let dependencies = cross_boundary_dependencies(resource, snapshot);
            if !dependencies.is_empty()
                && publish(
                    store,
                    context.recommendation(
                        resource,
                        DecisionKind::Network,
                        json!({
                            "operation": "co_locate_or_price_egress",
                            "cross_boundary_dependencies": dependencies,
                            "estimated_egress_usd": Value::Null,
                        }),
                        "Cross-provider or cross-region dependencies require co-location or an explicit dynamic egress quote",
                        ActionRisk::FinancialCommitment,
                    ),
                )
                .await?
            {
                summary.network += true as usize;
            }
        }
    }
    if policy.budgets.max_commitment_usd.is_some() {
        let stable_hourly: f64 = snapshot
            .resources
            .iter()
            .filter(|resource| resource.resource_type == "instance")
            .filter_map(|resource| resource.current_hourly_cost_usd)
            .sum();
        if stable_hourly > f64::default() {
            let synthetic = ResourceRecord::new(
                crate::capabilities::ProviderId::Stado,
                "global",
                "commitment_portfolio",
                "stable-compute",
                "stable-compute",
                now,
            );
            if publish(
                store,
                context.recommendation(
                    &synthetic,
                    DecisionKind::Commitment,
                    json!({
                        "stable_hourly_usd": stable_hourly,
                        "maximum_commitment_usd": policy.budgets.max_commitment_usd,
                        "operation": "obtain provider-native reservation quote and require an operator-approved immutable plan",
                    }),
                    "Stable on-demand usage may justify a commitment, but purchasing remains financially gated",
                    ActionRisk::FinancialCommitment,
                ),
            )
            .await?
            {
                summary.commitments += true as usize;
            }
        }
    }
    Ok(summary)
}

impl RecommendationContext<'_> {
    fn recommendation(
        &self,
        resource: &ResourceRecord,
        kind: DecisionKind,
        selected: serde_json::Value,
        explanation: &str,
        risk: ActionRisk,
    ) -> DecisionRecord {
        let authorization = self.policy.authorize(
            resource,
            risk,
            self.snapshot.complete,
            resource.current_hourly_cost_usd,
        );
        let expires =
            self.now + chrono::Duration::seconds(self.policy.limits.decision_ttl_seconds as i64);
        DecisionRecord {
            schema_version: SCHEMA_VERSION,
            decision_id: deterministic_id(resource, self.policy, self.snapshot, kind),
            kind,
            subject_id: resource.resource_id.clone(),
            created_at: self.now.to_rfc3339(),
            expires_at: expires.to_rfc3339(),
            inventory_snapshot_id: self.snapshot.snapshot_id.clone(),
            policy_version: self.policy.policy_version.clone(),
            selected: Some(selected),
            candidates: Vec::new(),
            constraints: vec![authorization.reason.clone()],
            explanation: explanation.to_string(),
            lease_token: None,
            state: if authorization.allowed {
                "authorized_recommendation".to_string()
            } else {
                "blocked_recommendation".to_string()
            },
        }
    }
}

async fn publish(store: &JobStorage, decision: DecisionRecord) -> Result<bool, StorageError> {
    match super::storage::write_decision(store, &decision).await {
        Ok(()) => Ok(true),
        Err(StorageError::StorageConflict(_)) => Ok(false),
        Err(error) => Err(error),
    }
}

fn deterministic_id(
    resource: &ResourceRecord,
    policy: &AutonomyPolicy,
    snapshot: &InventorySnapshot,
    kind: DecisionKind,
) -> String {
    let payload = format!(
        "{}|{}|{}|{:?}",
        resource.resource_id, policy.policy_version, snapshot.snapshot_id, kind
    );
    format!("advice-{}", hex::encode(Sha256::digest(payload.as_bytes())))
}

fn cross_boundary_dependencies(
    resource: &ResourceRecord,
    snapshot: &InventorySnapshot,
) -> Vec<Value> {
    resource
        .dependencies
        .iter()
        .filter_map(|dependency_id| {
            snapshot
                .resources
                .iter()
                .find(|candidate| candidate.resource_id == *dependency_id)
        })
        .filter(|dependency| {
            dependency.provider != resource.provider
                || resource
                    .region
                    .as_deref()
                    .zip(dependency.region.as_deref())
                    .is_some_and(|(left, right)| left != right)
        })
        .map(|dependency| {
            json!({
                "resource_id": dependency.resource_id,
                "provider": dependency.provider,
                "region": dependency.region,
            })
        })
        .collect()
}

fn underutilized(resource: &ResourceRecord) -> bool {
    let samples = [
        utilization(resource, &["cpu_peak", "cpu", "cpu_max"]),
        utilization(resource, &["memory_peak", "memory", "memory_max"]),
        utilization(resource, &["gpu_peak", "gpu", "gpu_max"]),
    ];
    samples
        .into_iter()
        .flatten()
        .all(|value| normalize_utilization(value) < QUARTER)
        && samples.into_iter().any(|sample| sample.is_some())
}

fn utilization(resource: &ResourceRecord, keys: &[&str]) -> Option<f64> {
    keys.iter()
        .find_map(|key| resource.utilization.get(*key).copied())
        .filter(|value| value.is_finite() && *value >= f64::default())
}

fn normalize_utilization(value: f64) -> f64 {
    if value > (true as u8) as f64 {
        value / PERCENT
    } else {
        value
    }
}

fn storage_candidate(
    resource: &ResourceRecord,
    policy: &AutonomyPolicy,
    now: DateTime<Utc>,
) -> bool {
    if resource.workload.is_some()
        || !matches!(
            resource.resource_type.as_str(),
            "persistent_disk" | "managed_disk" | "volume"
        )
    {
        return false;
    }
    resource
        .created_at
        .as_deref()
        .and_then(|created| DateTime::parse_from_rfc3339(created).ok())
        .is_some_and(|created| {
            now.signed_duration_since(created.with_timezone(&Utc))
                .num_seconds()
                >= i64::try_from(policy.idle.disk_days * crate::monitor::billing::SECONDS_PER_DAY)
                    .unwrap_or(i64::MAX)
        })
}
