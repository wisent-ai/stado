//! Per-probe highlights: the one or two fields of each GCP inventory probe an
//! operator reads first, printed under the probe's own line.

pub(super) fn print_probe_highlights(probe: &crate::providers::gcp::inventory::ProbeReport) {
    match probe.name.as_str() {
        "billing_account" => {
            if let Some(enabled) = probe.detail.get("billing_enabled") {
                println!("  billing_enabled: {enabled}");
            }
        }
        "caller_permissions" => {
            if let Some(missing) = probe
                .detail
                .get("missing")
                .and_then(serde_json::Value::as_array)
            {
                if !missing.is_empty() {
                    println!(
                        "  missing permissions: {}",
                        missing
                            .iter()
                            .filter_map(serde_json::Value::as_str)
                            .collect::<Vec<_>>()
                            .join(", ")
                    );
                }
            }
        }
        "service_account_roles" => {
            if let Some(missing) = probe.detail.get("missing_by_service_account") {
                println!("  missing runtime roles by account: {missing}");
            }
        }
        "compute_instances" => {
            if let Some(statuses) = probe.detail.get("by_status") {
                println!("  statuses: {statuses}");
            }
            if let Some(instances) = probe
                .detail
                .get("instances")
                .and_then(serde_json::Value::as_array)
            {
                for instance in instances.iter().filter(|instance| {
                    instance
                        .get("status")
                        .and_then(serde_json::Value::as_str)
                        .is_some_and(|status| {
                            matches!(
                                status,
                                "RUNNING"
                                    | "STAGING"
                                    | "PROVISIONING"
                                    | "REPAIRING"
                                    | "STOPPING"
                                    | "SUSPENDING"
                            )
                        })
                }) {
                    println!(
                        "  active VM: name={}, zone={}, status={}, machine={}, accelerators={}",
                        instance.get("name").unwrap_or(&serde_json::Value::Null),
                        instance.get("zone").unwrap_or(&serde_json::Value::Null),
                        instance.get("status").unwrap_or(&serde_json::Value::Null),
                        instance
                            .get("machine_type")
                            .unwrap_or(&serde_json::Value::Null),
                        instance
                            .get("accelerators")
                            .unwrap_or(&serde_json::Value::Null),
                    );
                }
            }
        }
        "compute_disks" => {
            println!(
                "  disk_gb={}, unattached={}",
                probe
                    .detail
                    .get("total_gb")
                    .unwrap_or(&serde_json::Value::Null),
                probe
                    .detail
                    .get("unattached")
                    .unwrap_or(&serde_json::Value::Null),
            );
        }
        "managed_instance_groups" => {
            if let Some(target) = probe.detail.get("target_instances") {
                println!("  desired instances across MIGs: {target}");
            }
        }
        "cloud_run_service" => {
            if let Some(revision) = probe.detail.get("latest_ready_revision") {
                println!("  latest ready revision: {revision}");
            }
            if let Some(environment) = probe.detail.get("environment") {
                println!("  non-secret runtime config: {environment}");
            }
        }
        _ if probe.name.starts_with("compute_region_quota_") => {
            if let Some(exhausted) = probe.detail.get("exhausted") {
                println!("  exhausted quota: {exhausted}");
            }
        }
        _ => {}
    }
}
