//! Persistent disks, including the unattached ones nobody is paying attention
//! to but everybody is paying for.

use serde_json::{json, Value};

use crate::providers::gcp::inventory::fields::{aggregated, number_u64, tail, text};

pub(in crate::providers::gcp::inventory) fn disks_detail(
    value: &Value,
) -> (&'static str, Option<usize>, Value) {
    let disks: Vec<Value> = aggregated(value, "disks")
        .into_iter()
        .map(|disk| {
            let users = disk
                .get("users")
                .and_then(Value::as_array)
                .map_or(usize::default(), Vec::len);
            json!({
                "name": disk.get("name"),
                "id": disk.get("id"),
                "zone": tail(text(disk.get("zone"))),
                "region": tail(text(disk.get("region"))),
                "status": disk.get("status"),
                "size_gb": disk.get("sizeGb"),
                    "type_url": disk.get("type"),
                "type": tail(text(disk.get("type"))),
                "users": users,
                "unattached": users == usize::default(),
                "creation_timestamp": disk.get("creationTimestamp"),
                "fingerprint": disk.get("labelFingerprint"),
                "labels": disk.get("labels"),
                "description": disk.get("description"),
                "replica_zones": disk.get("replicaZones"),
                "resource_policies": disk.get("resourcePolicies"),
                "physical_block_size_bytes": disk.get("physicalBlockSizeBytes"),
            })
        })
        .collect();
    let total_gb: u64 = disks
        .iter()
        .filter_map(|disk| disk.get("size_gb").and_then(number_u64))
        .sum();
    let unattached = disks
        .iter()
        .filter(|disk| disk.get("unattached").and_then(Value::as_bool) == Some(true))
        .count();
    let count = disks.len();
    (
        "ok",
        Some(count),
        json!({"total_gb": total_gb, "unattached": unattached, "disks": disks}),
    )
}
