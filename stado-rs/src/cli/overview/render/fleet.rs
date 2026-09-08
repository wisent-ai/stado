//! The fleet section: how much of the declared fleet is publishing capacity,
//! the claimability verdict, one line per live capacity row, what each host
//! has been measured able to do, and which declared local hosts are silent.

use serde_json::Value;

use crate::deploy::fleet_claim::FleetClaim;

pub(super) fn print_fleet(document: &Value, claim: &FleetClaim) {
    let fleet = &document["fleet"];
    println!(
        "fleet: {} of {} local hosts publishing capacity | {} registered targets",
        fleet["publishing_capacity"],
        fleet["registered_local_workers"],
        fleet["registered_targets"]
    );
    // Nothing when the queue is moving, or when it is empty: a report that
    // prints a verdict every time is a report whose verdict stops being read.
    for line in claim.lines() {
        println!("{line}");
    }
    if let Some(workers) = fleet.get("workers").and_then(Value::as_array) {
        for worker in workers {
            let target = worker
                .get("target")
                .and_then(Value::as_str)
                .unwrap_or("unmapped");
            let version = worker
                .get("stado_version")
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            let kind = worker
                .get("kind")
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            println!("  worker: {target} [{kind}] stado={version}");
        }
    }
    if let Some(targets) = fleet.get("targets").and_then(Value::as_array) {
        for target in targets {
            let name = target
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("unnamed");
            let Some(measurement) = target.get("measurement") else {
                continue;
            };
            if measurement.is_null() {
                println!("  can do: {name} nothing has measured this host");
                continue;
            }
            let capabilities = measurement
                .get("capabilities")
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default();
            let rendered: Vec<String> = capabilities
                .iter()
                .map(|(id, entry)| {
                    let value = entry.get("value").and_then(Value::as_bool).unwrap_or(false);
                    format!("{id}={value}")
                })
                .collect();
            let measured_at = measurement
                .get("measured_at")
                .and_then(Value::as_str)
                .unwrap_or("unstamped");
            println!(
                "  can do: {name} {} (measured {measured_at})",
                rendered.join(" ")
            );
        }
    }
    if let Some(targets) = fleet.get("targets").and_then(Value::as_array) {
        let offline: Vec<&str> = targets
            .iter()
            .filter(|target| target.get("active_worker").and_then(Value::as_bool) == Some(false))
            .filter_map(|target| target.get("name").and_then(Value::as_str))
            .collect();
        if !offline.is_empty() {
            println!("  offline local: {}", offline.join(", "));
        }
    }
}
