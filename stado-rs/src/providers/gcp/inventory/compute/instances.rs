//! Instances and the groups and reservations that stand behind them.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use crate::providers::gcp::inventory::fields::{aggregated, number_u64, tail, tail_of};
use crate::providers::gcp::inventory::{Detail, ProbeState};

pub(in crate::providers::gcp::inventory) fn instances_detail(value: &Value) -> Detail {
    let mut instances = Vec::new();
    let mut by_status = BTreeMap::<String, Vec<String>>::new();
    for item in aggregated(value, "instances") {
        // An instance listed without its status or name is grouped and
        // named as that, not under "" as an instance with an empty status.
        let status = item.get("status").and_then(Value::as_str);
        let named = |field: &str| match item.get(field).and_then(Value::as_str) {
            Some(text) => text.to_string(),
            None => format!("(no {field} listed)"),
        };
        by_status
            .entry(named("status"))
            .or_default()
            .push(named("name"));
        let accelerators: Vec<Value> = item
            .get("guestAccelerators")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .map(|accelerator| {
                json!({
                    "type": tail_of(accelerator.get("acceleratorType")),
                    "count": accelerator.get("acceleratorCount"),
                })
            })
            .collect();
        let creator = item
            .get("metadata")
            .and_then(|metadata| metadata.get("items"))
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .find(|entry| entry.get("key").and_then(Value::as_str) == Some("created-by"))
            .and_then(|entry| entry.get("value"))
            .and_then(Value::as_str)
            .map(tail);
        let disk_gb: u64 = item
            .get("disks")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|disk| disk.get("diskSizeGb").and_then(number_u64))
            .sum();
        instances.push(json!({
            "name": item.get("name"),
            "instance_id": item.get("id"),
            "zone": tail_of(item.get("zone")),
            "status": status,
            "machine_type": tail_of(item.get("machineType")),
            "accelerators": accelerators,
            "provisioning_model": item.pointer("/scheduling/provisioningModel"),
            "preemptible": item.pointer("/scheduling/preemptible"),
            "created_by": creator,
            "stado_managed": stado_agent(item),
            "disk_gb": disk_gb,
            "creation_timestamp": item.get("creationTimestamp"),
        }));
    }
    let count = instances.len();
    (
        ProbeState::Ok,
        Some(count),
        json!({"by_status": by_status, "instances": instances}),
    )
}

/// The resource carries the name every Stado agent VM is launched under
/// (`<INSTANCE_PREFIX>-agent-…`), which its boot disk shares.
pub(in crate::providers::gcp::inventory) fn stado_agent(item: &Value) -> bool {
    let prefix = format!("{}-agent-", crate::config::INSTANCE_PREFIX);
    item.get("name")
        .and_then(Value::as_str)
        .is_some_and(|name| name.starts_with(&prefix))
}

pub(in crate::providers::gcp::inventory) fn instance_groups_detail(value: &Value) -> Detail {
    let groups: Vec<Value> = aggregated(value, "instanceGroupManagers")
        .into_iter()
        .map(|group| {
            json!({
                "name": group.get("name"),
                "id": group.get("id"),
                "zone": tail_of(group.get("zone")),
                "region": tail_of(group.get("region")),
                "target_size": group.get("targetSize"),
                "instance_template": tail_of(group.get("instanceTemplate")),
                "stable": group.pointer("/status/isStable"),
                "version_target_reached": group.pointer("/status/versionTarget/isReached"),
                "creation_timestamp": group.get("creationTimestamp"),
                "fingerprint": group.get("fingerprint"),
            })
        })
        .collect();
    let target_instances: u64 = groups
        .iter()
        .filter_map(|group| group.get("target_size").and_then(number_u64))
        .sum();
    let count = groups.len();
    (
        ProbeState::Ok,
        Some(count),
        json!({"target_instances": target_instances, "managed_instance_groups": groups}),
    )
}

pub(in crate::providers::gcp::inventory) fn reservations_detail(value: &Value) -> Detail {
    let reservations: Vec<Value> = aggregated(value, "reservations")
        .into_iter()
        .map(|reservation| {
            json!({
                "name": reservation.get("name"),
                "id": reservation.get("id"),
                "zone": tail_of(reservation.get("zone")),
                "status": reservation.get("status"),
                "specific_reservation": reservation.get("specificReservation"),
                "in_use_count": reservation
                    .pointer("/specificReservation/inUseCount")
                    .and_then(number_u64),
                "specific_reservation_required": reservation.get("specificReservationRequired"),
                "creation_timestamp": reservation.get("creationTimestamp"),
                "self_link": reservation.get("selfLink"),
            })
        })
        .collect();
    let count = reservations.len();
    (
        ProbeState::Ok,
        Some(count),
        json!({"reservations": reservations}),
    )
}
