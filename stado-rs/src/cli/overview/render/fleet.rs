//! The fleet section: how much of the declared fleet is publishing capacity,
//! the claimability verdict, one line per live capacity row, what each host
//! has been measured able to do, and which declared local hosts are silent.

use serde_json::Value;

use crate::cli::reporting::field;
use crate::deploy::fleet_claim::FleetClaim;

pub(super) fn print_fleet(document: &Value, claim: &FleetClaim) {
    let fleet = &document["fleet"];
    println!(
        "fleet: {} of {} local hosts publishing capacity | {} registered targets",
        field(&fleet["publishing_capacity"]),
        field(&fleet["registered_local_workers"]),
        field(&fleet["registered_targets"])
    );
    // Nothing when the queue is moving, or when it is empty: a report that
    // prints a verdict every time is a report whose verdict stops being read.
    for line in claim.lines() {
        println!("{line}");
    }
    if let Some(workers) = fleet.get("workers").and_then(Value::as_array) {
        for worker in workers {
            // A row the registry maps to no target is named by the consumer
            // that published it, so the operator can find which host it is.
            let target = match worker["target"].as_str() {
                Some(target) => target.to_string(),
                None => format!("unmapped consumer {}", field(&worker["consumer_id"])),
            };
            println!(
                "  worker: {target} [kind {}] stado={}",
                field(&worker["kind"]),
                field(&worker["stado_version"])
            );
        }
    }
    if let Some(targets) = fleet.get("targets").and_then(Value::as_array) {
        for target in targets {
            let name = field(&target["name"]);
            let Some(measurement) = target.get("measurement") else {
                continue;
            };
            if measurement.is_null() {
                println!("  can do: {name} nothing has measured this host");
                continue;
            }
            if let Some(reason) = measurement.get("unreadable").and_then(Value::as_str) {
                println!("  can do: {name} its measurement cannot be read: {reason}");
                continue;
            }
            let rendered: Vec<String> = match measurement["capabilities"].as_object() {
                Some(capabilities) => capabilities
                    .iter()
                    .map(|(id, entry)| format!("{id}={}", field(&entry["value"])))
                    .collect(),
                None => vec![format!(
                    "capabilities {}",
                    field(&measurement["capabilities"])
                )],
            };
            let measured_at = field(&measurement["measured_at"]);
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
