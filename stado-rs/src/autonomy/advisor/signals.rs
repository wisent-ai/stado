//! The questions the advisory walk asks about one resource.
//!
//! `cross_boundary_dependencies` lists the dependencies that sit on another
//! provider or in another region, `underutilized` and `utilization` read the
//! peak samples the inventory carries, `normalize_utilization` accepts either
//! a ratio or a percentage, and `storage_candidate` decides whether an
//! unattached volume aged past the lifecycle window.

use chrono::{DateTime, Utc};
use serde_json::{json, Value};

use crate::autonomy::model::{InventorySnapshot, ResourceRecord};
use crate::autonomy::policy::AutonomyPolicy;

const TWO: u8 = (u16::BITS / u8::BITS) as u8;
const QUARTER: f64 = (true as u8) as f64 / (TWO * TWO) as f64;
const PERCENT: f64 = ((u8::BITS as u8 + TWO) * (u8::BITS as u8 + TWO)) as f64;

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

pub(super) fn underutilized(resource: &ResourceRecord) -> bool {
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

pub(super) fn utilization(resource: &ResourceRecord, keys: &[&str]) -> Option<f64> {
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
            now.signed_duration_since(created.with_timezone(&Utc))
                .num_seconds()
                >= i64::try_from(policy.idle.disk_days * crate::monitor::billing::SECONDS_PER_DAY)
                    .unwrap_or(i64::MAX)
        })
}
