//! Instances and the groups and reservations that stand behind them.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use crate::providers::gcp::inventory::fields::{aggregated, number_u64, tail, text};

pub(in crate::providers::gcp::inventory) fn instances_detail(
    value: &Value,
) -> (&'static str, Option<usize>, Value) {
    let mut instances = Vec::new();
    let mut by_status = BTreeMap::<String, usize>::new();
    for item in aggregated(value, "instances") {
        let status = text(item.get("status"));
        *by_status.entry(status.clone()).or_default() += true as usize;
        let accelerators: Vec<Value> = item
            .get("guestAccelerators")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .map(|accelerator| {
                json!({
                    "type": tail(text(accelerator.get("acceleratorType"))),
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
            "zone": tail(text(item.get("zone"))),
            "status": status,
            "machine_type": tail(text(item.get("machineType"))),
            "accelerators": accelerators,
            "provisioning_model": item.pointer("/scheduling/provisioningModel"),
            "created_by": creator,
            "stado_managed": item.get("name").and_then(Value::as_str).is_some_and(|name| name.starts_with("wisent-agent-")),
            "disk_gb": disk_gb,
            "creation_timestamp": item.get("creationTimestamp"),
        }));
    }
    let count = instances.len();
    (
        "ok",
        Some(count),
        json!({"by_status": by_status, "instances": instances}),
    )
}

pub(in crate::providers::gcp::inventory) fn instance_groups_detail(
    value: &Value,
) -> (&'static str, Option<usize>, Value) {
    let groups: Vec<Value> = aggregated(value, "instanceGroupManagers")
        .into_iter()
        .map(|group| {
            json!({
                "name": group.get("name"),
                "id": group.get("id"),
                "zone": tail(text(group.get("zone"))),
                "region": tail(text(group.get("region"))),
                "target_size": group.get("targetSize"),
                "instance_template": tail(text(group.get("instanceTemplate"))),
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
        "ok",
        Some(count),
        json!({"target_instances": target_instances, "managed_instance_groups": groups}),
    )
}

pub(in crate::providers::gcp::inventory) fn reservations_detail(
    value: &Value,
) -> (&'static str, Option<usize>, Value) {
    let reservations: Vec<Value> = aggregated(value, "reservations")
        .into_iter()
        .map(|reservation| {
            json!({
                "name": reservation.get("name"),
                "id": reservation.get("id"),
                "zone": tail(text(reservation.get("zone"))),
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
    ("ok", Some(count), json!({"reservations": reservations}))
}
