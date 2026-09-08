//! The fleet section: the declared registry joined to the hosts that are
//! actually publishing capacity, plus what each host has been measured able
//! to do.
//!
//! The join goes through the hostname, because a capacity row is keyed by
//! consumer id and a registry target by name; `target_for_consumer` is the
//! one place that reconciles the two spellings.

use std::collections::HashSet;

use serde_json::{json, Value};

use crate::deploy::fleet_claim::FleetClaim;
use crate::targets::{self, ComputeTarget, Registry};

fn target_identities(target: &ComputeTarget) -> HashSet<String> {
    let mut identities = HashSet::from([targets::normalize_hostname(&target.name)]);
    identities.extend(
        target
            .hostnames
            .iter()
            .map(|name| targets::normalize_hostname(name)),
    );
    identities
}

fn target_for_consumer<'a>(
    registry: &'a Registry,
    consumer_id: &str,
    kind: &str,
) -> Option<&'a str> {
    let hostname = consumer_id
        .strip_prefix(&format!("{kind}-"))
        .unwrap_or(consumer_id);
    let hostname = targets::normalize_hostname(hostname);
    registry
        .targets
        .iter()
        .find(|target| target.kind == kind && target_identities(target).contains(&hostname))
        .map(|target| target.name.as_str())
}

pub(super) fn fleet_snapshot(
    registry: &Registry,
    consumers: &std::collections::BTreeMap<String, Value>,
    measurements: &std::collections::BTreeMap<String, crate::cli::registry::Measurement>,
    claim: &FleetClaim,
) -> Value {
    let mut active_targets = HashSet::new();
    let workers: Vec<Value> = consumers
        .iter()
        .map(|(consumer_id, payload)| {
            let kind = payload
                .get("kind")
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            let target = target_for_consumer(registry, consumer_id, kind);
            if let Some(name) = target {
                active_targets.insert(name.to_string());
            }
            json!({
                "consumer_id": consumer_id,
                "target": target,
                "kind": kind,
                "published_at": payload.get("published_at").cloned().unwrap_or(Value::Null),
                "stado_version": payload.get("stado_version").cloned().unwrap_or(Value::Null),
                "accepting_jobs": payload.get("accepting_jobs").cloned().unwrap_or(Value::Null),
                "running_jobs": payload.get("running_jobs").cloned().unwrap_or(Value::Null),
                "available_cpu_cores": payload.get("available_cpu_cores").cloned().unwrap_or(Value::Null),
                "total_cpu_cores": payload.get("total_cpu_cores").cloned().unwrap_or(Value::Null),
                "available_accelerators": payload
                    .get("available_accelerators")
                    .cloned()
                    .unwrap_or_else(|| json!({})),
                "free_ram_gb": payload.get("free_ram_gb").cloned().unwrap_or(Value::Null),
                "total_ram_gb": payload.get("total_ram_gb").cloned().unwrap_or(Value::Null),
                "free_vram_gb": payload.get("free_vram_gb").cloned().unwrap_or(Value::Null),
                "total_vram_gb": payload.get("total_vram_gb").cloned().unwrap_or(Value::Null),
            })
        })
        .collect();

    let targets: Vec<Value> = registry
        .targets
        .iter()
        .map(|target| {
            let active_worker = target
                .is_provider(crate::capabilities::ProviderId::Local)
                .then(|| active_targets.contains(&target.name));
            let measured = measurements.get(&target.name).map(|measurement| {
                let values: serde_json::Map<String, Value> = measurement
                    .capabilities
                    .iter()
                    .map(|(id, capability)| {
                        (
                            id.clone(),
                            json!({"value": capability.value, "detail": capability.detail}),
                        )
                    })
                    .collect();
                json!({
                    "measured_at": measurement.measured_at.map(|stamp| stamp.to_rfc3339()),
                    "capabilities": Value::Object(values),
                })
            });
            json!({
                "name": target.name,
                "kind": target.kind,
                "active_worker": active_worker,
                "gpu_type": target.gpu_type,
                "pinned_only": target.pinned_only,
                // Absent means nothing has measured this host, which is a
                // different statement from a capability measured false.
                "measurement": measured,
            })
        })
        .collect();
    let coordinators: Vec<Value> = registry
        .coordinators
        .iter()
        .map(|coordinator| {
            json!({
                "name": coordinator.name,
                "runtime": coordinator.runtime,
                "active": coordinator.active,
                "interval_seconds": coordinator.interval_seconds,
            })
        })
        .collect();
    let local_registered = registry
        .targets
        .iter()
        .filter(|target| target.is_provider(crate::capabilities::ProviderId::Local))
        .count();

    json!({
        // How many DECLARED LOCAL HOSTS published capacity inside the
        // staleness horizon -- not how many rows are in the capacity prefix,
        // and emphatically not how many workers the registry declares. The
        // key used to be `active_workers`, a count of live broadcast rows
        // printed under the words "active workers", which read as a healthy
        // fleet on a day when nothing in it could claim anything.
        "publishing_capacity": claim.publishing.len(),
        "capacity_rows_live": workers.len(),
        "registered_targets": targets.len(),
        "registered_local_workers": local_registered,
        "workers": workers,
        "targets": targets,
        "coordinators": coordinators,
    })
}
