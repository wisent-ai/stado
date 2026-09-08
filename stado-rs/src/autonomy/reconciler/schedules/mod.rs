//! Reconciling the start and stop expressions a resource rule declares.
//!
//! [`marker`] holds the once-per-occurrence claim and [`plan`] builds the
//! single reversible action a due expression turns into.

mod marker;
mod plan;

// `policy` is rebound here for the moved signature that names
// `super::policy::ResourceRule` verbatim in `plan`.
use super::policy;

use std::collections::BTreeMap;

use chrono::Utc;
use serde_json::json;
use sha2::{Digest, Sha256};

use crate::cli::resources::model::ActionKind;
use crate::queue::{JobStorage, StorageError};

use crate::autonomy::model::{InventorySnapshot, Ownership};

use super::mutations::execute_with_circuit;
use super::policy::AutonomyPolicy;
use marker::acquire_schedule_marker;
use plan::schedule_plan;

pub(super) async fn reconcile_schedules(
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
