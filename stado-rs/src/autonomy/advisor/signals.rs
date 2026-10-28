//! The questions the advisory walk asks about one resource.
//!
//! `cross_boundary_dependencies` lists the dependencies that sit on another
//! provider or in another region, `underutilized` and `utilization` read the
//! peak samples the inventory carries as ratios of capacity, and
//! `storage_candidate` decides whether an unattached volume aged past the
//! lifecycle window.

use chrono::{DateTime, Utc};
use serde_json::{json, Value};

use crate::autonomy::model::{InventorySnapshot, ResourceKind, ResourceRecord};
use crate::autonomy::policy::AutonomyPolicy;
use crate::capabilities::ProviderId;

/// One immutable snapshot, with its ordering observed only when a lookup needs it.
pub(super) struct ResourceLookup<'a> {
    snapshot: &'a InventorySnapshot,
    sorted: Option<bool>,
}

impl<'a> ResourceLookup<'a> {
    pub(super) fn new(snapshot: &'a InventorySnapshot) -> Self {
        Self {
            snapshot,
            sorted: None,
        }
    }

    fn resource(&mut self, id: &str) -> Option<&'a ResourceRecord> {
        let resources = self.snapshot.resources.as_slice();
        let sorted = *self.sorted.get_or_insert_with(|| {
            resources
                .is_sorted_by(|left, right| left.resource_id.as_str() <= right.resource_id.as_str())
        });
        if sorted {
            let index = resources.partition_point(|resource| resource.resource_id.as_str() < id);
            resources
                .get(index)
                .filter(|resource| resource.resource_id.as_str() == id)
        } else {
            resources
                .iter()
                .find(|resource| resource.resource_id.as_str() == id)
        }
    }
}

/// Refuse an unresolved declared dependency instead of reporting an incomplete graph.
pub(super) fn cross_boundary_dependencies(
    resource: &ResourceRecord,
    lookup: &mut ResourceLookup<'_>,
) -> Result<Vec<Value>, String> {
    resource
        .dependencies
        .iter()
        .map(|dependency_id| {
            let dependency = lookup.resource(dependency_id).ok_or_else(|| {
                format!(
                    "network dependency {dependency_id} is absent from inventory snapshot {}",
                    lookup.snapshot.snapshot_id
                )
            })?;
            let cross_boundary = dependency.provider != resource.provider
                || resource
                    .region
                    .as_deref()
                    .zip(dependency.region.as_deref())
                    .is_some_and(|(left, right)| left != right);
            Ok(cross_boundary.then(|| {
                json!({
                    "resource_id": dependency.resource_id,
                    "provider": dependency.provider,
                    "region": dependency.region,
                })
            }))
        })
        .filter_map(Result::transpose)
        .collect()
}

pub(super) struct UtilizationPeaks {
    pub(super) cpu: Option<f64>,
    pub(super) memory: Option<f64>,
    pub(super) gpu: Option<f64>,
}

/// Every observed peak sits below the policy's `idle.underutilized_below`
/// ratio, and at least one peak was observed. A policy that states no ratio
/// names nothing underutilized.
pub(super) fn underutilized(
    resource: &ResourceRecord,
    policy: &AutonomyPolicy,
) -> Result<Option<UtilizationPeaks>, String> {
    let Some(ratio) = policy.idle.underutilized_below else {
        return Ok(None);
    };
    let peaks = UtilizationPeaks {
        cpu: utilization(resource, &["cpu_peak", "cpu", "cpu_max"])?,
        memory: utilization(resource, &["memory_peak", "memory", "memory_max"])?,
        gpu: utilization(resource, &["gpu_peak", "gpu", "gpu_max"])?,
    };
    let mut observed = false;
    let below = [peaks.cpu, peaks.memory, peaks.gpu]
        .into_iter()
        .flatten()
        .all(|value| {
            observed = true;
            value < ratio
        });
    Ok((below && observed).then_some(peaks))
}

fn utilization(resource: &ResourceRecord, keys: &[&str]) -> Result<Option<f64>, String> {
    let Some((key, value)) = keys
        .iter()
        .find_map(|key| resource.utilization.get(*key).map(|value| (*key, *value)))
    else {
        return Ok(None);
    };
    if value.is_finite() && !value.is_sign_negative() {
        Ok(Some(value))
    } else {
        Err(format!(
            "utilization metric {key:?} must be finite and non-negative; observed {value}"
        ))
    }
}

/// Whether an unattached disk has sat past the policy's `idle.disk_days`.
/// A disk whose age cannot be told — no creation time, one that is not
/// RFC 3339, or a day count no duration holds — is the refusal that says
/// which, so the pass names it instead of leaving it out unseen.
/// Attachment evidence must affirm that the disk is unused before its age is judged.
pub(super) fn storage_candidate(
    resource: &ResourceRecord,
    policy: &AutonomyPolicy,
    now: DateTime<Utc>,
) -> Result<bool, String> {
    if resource.workload.is_some() || !resource.resource_type.disk() || !unattached(resource)? {
        return Ok(false);
    }
    let created = resource
        .created_at
        .as_deref()
        .ok_or_else(|| "it records no creation time, so its age cannot be told".to_string())?;
    let created = DateTime::parse_from_rfc3339(created)
        .map_err(|error| format!("its creation time {created} is not RFC 3339: {error}"))?;
    let days = i64::try_from(policy.idle.disk_days).map_err(|error| {
        format!(
            "the policy's idle.disk_days {} is not a day count: {error}",
            policy.idle.disk_days
        )
    })?;
    let window = chrono::Duration::try_days(days).ok_or_else(|| {
        format!("the policy's idle.disk_days {days} is longer than a duration holds")
    })?;
    Ok(now.signed_duration_since(created.with_timezone(&Utc)) >= window)
}

fn unattached(resource: &ResourceRecord) -> Result<bool, String> {
    match resource.provider {
        ProviderId::Aws => match resource.state.as_deref() {
            Some(state) if !state.is_empty() => {
                Ok(state == aws_sdk_ec2::types::VolumeState::Available.as_str())
            }
            observed => Err(format!(
                "storage lifecycle needs an AWS volume state; observed {observed:?}"
            )),
        },
        ProviderId::Gcp => match resource.evidence.pointer("/item/unattached") {
            Some(Value::Bool(unattached)) => Ok(*unattached),
            observed => Err(format!(
                "storage lifecycle needs a boolean GCP item.unattached observation; observed {observed:?}"
            )),
        },
        ProviderId::Azure => match resource.evidence.pointer("/properties/diskState") {
            Some(Value::String(state)) if !state.is_empty() => Ok(state == "Unattached"),
            observed => Err(format!(
                "storage lifecycle needs an Azure properties.diskState observation; observed {observed:?}"
            )),
        },
        provider => Err(format!(
            "storage lifecycle has no attachment-state interpretation for provider {}",
            provider.as_str()
        )),
    }
}

/// Price every observed instance or retain why the portfolio cannot be estimated.
pub(super) fn commitment_hourly(snapshot: &InventorySnapshot) -> Result<Option<f64>, String> {
    let mut total: Option<f64> = None;
    for resource in snapshot
        .resources
        .iter()
        .filter(|resource| resource.resource_type == ResourceKind::Instance)
    {
        let price = resource.current_hourly_cost_usd.ok_or_else(|| {
            format!(
                "{}: commitment estimate has no observed hourly price for this instance",
                resource.resource_id
            )
        })?;
        if !price.is_finite() || price.is_sign_negative() {
            return Err(format!(
                "{}: commitment hourly price must be finite and non-negative; observed {price}",
                resource.resource_id
            ));
        }
        let observed = match total {
            Some(subtotal) => subtotal + price,
            None => price,
        };
        if !observed.is_finite() {
            return Err(format!(
                "{}: commitment hourly total became {observed} after adding {price} USD/hour",
                resource.resource_id
            ));
        }
        total = Some(observed);
    }
    Ok(total)
}
