//! The questions the advisory walk asks about one resource.
//!
//! `cross_boundary_dependencies` lists the dependencies that sit on another
//! provider or in another region, `underutilized` and `utilization` read the
//! peak samples the inventory carries as ratios of capacity, and
//! `storage_candidate` decides whether an unattached volume aged past the
//! lifecycle window.

use chrono::{DateTime, Utc};
use serde_json::{json, Value};

use crate::autonomy::model::{InventorySnapshot, ResourceRecord};
use crate::autonomy::policy::AutonomyPolicy;

pub(super) fn cross_boundary_dependencies(
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

/// Every observed peak sits below the policy's `idle.underutilized_below`
/// ratio, and at least one peak was observed.
pub(super) fn underutilized(resource: &ResourceRecord, policy: &AutonomyPolicy) -> bool {
    let samples = [
        utilization(resource, &["cpu_peak", "cpu", "cpu_max"]),
        utilization(resource, &["memory_peak", "memory", "memory_max"]),
        utilization(resource, &["gpu_peak", "gpu", "gpu_max"]),
    ];
    samples
        .into_iter()
        .flatten()
        .all(|value| value < policy.idle.underutilized_below)
        && samples.into_iter().any(|sample| sample.is_some())
}

pub(super) fn utilization(resource: &ResourceRecord, keys: &[&str]) -> Option<f64> {
    keys.iter()
        .find_map(|key| resource.utilization.get(*key).copied())
        .filter(|value| value.is_finite() && !value.is_sign_negative())
}

pub(super) fn storage_candidate(
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
            let window = i64::try_from(policy.idle.disk_days)
                .ok()
                .and_then(chrono::Duration::try_days);
            window.is_some_and(|window| {
                now.signed_duration_since(created.with_timezone(&Utc)) >= window
            })
        })
}
