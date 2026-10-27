//! The advisory pass: one walk of the inventory, one publish per finding.
//!
//! `publish_recommendations` emits the rightsizing, schedule,
//! storage-lifecycle and network advice each resource justifies plus the
//! portfolio-wide commitment advice, and `publish` writes one decision,
//! treating an already-written identity as nothing new to count.

use chrono::{DateTime, Utc};
use serde_json::{json, Value};

use crate::autonomy::model::{
    DecisionKind, DecisionRecord, InventorySnapshot, ResourceKind, ResourceRecord,
};
use crate::autonomy::policy::{ActionRisk, AutonomyPolicy};
use crate::queue::{JobStorage, StorageError};

use super::decision::RecommendationContext;
use super::signals::{
    commitment_hourly, cross_boundary_dependencies, storage_candidate, underutilized,
    ResourceLookup,
};
use super::summary::AdvisorSummary;

pub async fn publish_recommendations(
    store: &JobStorage,
    snapshot: &InventorySnapshot,
    policy: &AutonomyPolicy,
    now: DateTime<Utc>,
) -> Result<AdvisorSummary, StorageError> {
    let mut summary = AdvisorSummary::default();
    let expires = crate::autonomy::time::expires_at(
        now,
        policy.limits.decision_ttl_seconds,
        "limits.decision_ttl_seconds",
    )
    .map_err(StorageError::Other)?;
    let context = RecommendationContext {
        snapshot,
        policy,
        now,
        expires,
    };
    let mut dependency_lookup = policy
        .placement
        .account_for_egress
        .then(|| ResourceLookup::new(snapshot));
    for resource in &snapshot.resources {
        let peaks = match underutilized(resource, policy) {
            Ok(peaks) => peaks,
            Err(why) => {
                summary
                    .unjudged
                    .push(format!("{}: {why}", resource.resource_id));
                None
            }
        };
        if let Some(peaks) = peaks.filter(|_| resource.current_hourly_cost_usd.is_some()) {
            if publish(
                store,
                context.recommendation(
                    resource,
                    DecisionKind::Rightsize,
                    json!({
                        "operation": "replace_on_next_idle_cycle",
                        "target": "smallest provider shape satisfying observed peak plus headroom",
                        "cpu_peak": peaks.cpu,
                        "memory_peak": peaks.memory,
                        "gpu_peak": peaks.gpu,
                    }),
                    "Resource stayed below the rightsizing utilization threshold; replace only after it is idle",
                    ActionRisk::Reversible,
                )?,
            )
            .await?
            {
                summary.rightsizing.push(resource.resource_id.clone());
            }
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
                    )?,
                )
                .await?
            {
                summary.schedules.push(resource.resource_id.clone());
            }
        }
        let aged = match storage_candidate(resource, policy, now) {
            Ok(aged) => aged,
            Err(why) => {
                summary
                    .unjudged
                    .push(format!("{}: {why}", resource.resource_id));
                false
            }
        };
        if aged
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
                )?,
            )
            .await?
        {
            summary.storage_lifecycle.push(resource.resource_id.clone());
        }
        if let Some(lookup) = dependency_lookup.as_mut() {
            match cross_boundary_dependencies(resource, lookup) {
                Ok(dependencies) => {
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
                            )?,
                        )
                        .await?
                    {
                        summary.network.push(resource.resource_id.clone());
                    }
                }
                Err(why) => summary
                    .unjudged
                    .push(format!("{}: {why}", resource.resource_id)),
            }
        }
    }
    if policy.budgets.max_commitment_usd.is_some() {
        match commitment_hourly(snapshot) {
            Ok(hourly) => {
                if let Some(stable_hourly) =
                    hourly.filter(|price| price.is_normal() && price.is_sign_positive())
                {
                    let synthetic = ResourceRecord::new(
                        crate::capabilities::ProviderId::Stado,
                        "global",
                        ResourceKind::CommitmentPortfolio,
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
                            "Observed priced compute usage may justify a commitment, but purchasing remains financially gated",
                            ActionRisk::FinancialCommitment,
                        )?,
                    )
                    .await?
                    {
                        summary.commitments.push(synthetic.resource_id.clone());
                    }
                }
            }
            Err(why) => summary.unjudged.push(why),
        }
    }
    Ok(summary)
}

async fn publish(store: &JobStorage, decision: DecisionRecord) -> Result<bool, StorageError> {
    match super::storage::write_decision(store, &decision).await {
        Ok(()) => Ok(true),
        Err(StorageError::StorageConflict(_)) => Ok(false),
        Err(error) => Err(error),
    }
}
