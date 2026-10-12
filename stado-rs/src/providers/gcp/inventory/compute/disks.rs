//! Persistent disks, including the unattached ones nobody is paying attention
//! to but everybody is paying for.

use serde_json::{json, Value};

use crate::providers::gcp::inventory::fields::{aggregated, number_u64, tail, text};
use crate::providers::gcp::inventory::{Detail, ProbeState};

use super::instances::stado_agent;

pub(in crate::providers::gcp::inventory) fn disks_detail(value: &Value) -> Detail {
    let disks: Vec<Value> = aggregated(value, "disks")
        .into_iter()
        .map(|disk| {
            let users = disk.get("users").and_then(Value::as_array);
            json!({
                "name": disk.get("name"),
                "id": disk.get("id"),
                "zone": tail(text(disk.get("zone"))),
                "region": tail(text(disk.get("region"))),
                "status": disk.get("status"),
                "size_gb": disk.get("sizeGb"),
                    "type_url": disk.get("type"),
                "type": tail(text(disk.get("type"))),
                "users": users.into_iter().flatten().count(),
                "unattached": users.is_none_or(Vec::is_empty),
                "creation_timestamp": disk.get("creationTimestamp"),
                "fingerprint": disk.get("labelFingerprint"),
                "labels": disk.get("labels"),
                "stado_managed": stado_agent(disk),
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
        ProbeState::Ok,
        Some(count),
        json!({"total_gb": total_gb, "unattached": unattached, "disks": disks}),
    )
}
