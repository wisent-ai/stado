//! Findings derived from the GCP inventory probes. Only probes that reported
//! `ok` contribute, so a partial inventory never reads as an empty project,
//! and every candidate must clear the audit grace period.

use chrono::{DateTime, Utc};
use serde_json::Value;

use crate::cli::resources::rationalize::Finding;
use crate::providers::gcp::inventory::{GcpInventoryReport, ProbeReport};

use super::fields::{finding, first_nonempty, number_field, old_enough, resource_at, string_field};

pub(super) fn gcp_findings(
    report: &GcpInventoryReport,
    min_age_seconds: u64,
    now: DateTime<Utc>,
    provider_disabled: bool,
) -> Vec<Finding> {
    let mut findings = Vec::new();

    for disk in detail_items(report, "compute_disks", "disks") {
        if disk.get("unattached").and_then(Value::as_bool) != Some(true)
            || !old_enough(disk.get("creation_timestamp"), min_age_seconds, now)
        {
            continue;
        }
        let name = string_field(disk, "name");
        let location = first_nonempty(&[string_field(disk, "zone"), string_field(disk, "region")]);
        findings.push(finding(
            &format!("gcp-unattached-disk-{name}"),
            "medium",
            "review-delete",
            "medium",
            crate::capabilities::ProviderId::Gcp.as_str(),
            "persistent-disk",
            resource_at(&name, &location),
            "the persistent disk is unattached and older than the audit grace period; inspect its labels and snapshots before deletion",
            disk.clone(),
        ));
    }

    for address in detail_items(report, "static_addresses", "addresses") {
        let users_empty = address
            .get("users")
            .and_then(Value::as_array)
            .is_none_or(Vec::is_empty);
        if address.get("status").and_then(Value::as_str) != Some("RESERVED")
            || !users_empty
            || !old_enough(address.get("creation_timestamp"), min_age_seconds, now)
        {
            continue;
        }
        let name = string_field(address, "name");
        let region = string_field(address, "region");
        findings.push(finding(
            &format!("gcp-unused-address-{name}"),
            "medium",
            "review-release",
            "medium",
            crate::capabilities::ProviderId::Gcp.as_str(),
            "static-address",
            resource_at(&name, &region),
            "the static address is reserved, has no users and is older than the audit grace period",
            address.clone(),
        ));
    }

    for group in detail_items(report, "managed_instance_groups", "managed_instance_groups") {
        let name = string_field(group, "name");
        let stado_owned = name.starts_with("wisent") || name.starts_with("stado");
        if !stado_owned
            || number_field(group, "target_size") != Some(u64::default())
            || !old_enough(group.get("creation_timestamp"), min_age_seconds, now)
        {
            continue;
        }
        let location =
            first_nonempty(&[string_field(group, "zone"), string_field(group, "region")]);
        findings.push(finding(
            &format!("gcp-empty-instance-group-{name}"),
            "medium",
            "review-delete",
            "medium",
            crate::capabilities::ProviderId::Gcp.as_str(),
            "managed-instance-group",
            resource_at(&name, &location),
            "the Stado/Wisent managed instance group has target size zero and is older than the audit grace period",
            group.clone(),
        ));
    }

    if provider_disabled {
        for reservation in detail_items(report, "compute_reservations", "reservations") {
            if !old_enough(reservation.get("creation_timestamp"), min_age_seconds, now) {
                continue;
            }
            let name = string_field(reservation, "name");
            let zone = string_field(reservation, "zone");
            findings.push(finding(
                &format!("gcp-disabled-provider-reservation-{name}"),
                "medium",
                "review-release",
                "medium",
                crate::capabilities::ProviderId::Gcp.as_str(),
                "compute-reservation",
                resource_at(&name, &zone),
                "GCP scheduling is disabled while this reservation remains; release it after confirming that no non-Stado workload consumes it",
                reservation.clone(),
            ));
        }
    }

    findings
}

fn detail_items<'a>(
    report: &'a GcpInventoryReport,
    probe_name: &str,
    key: &str,
) -> impl Iterator<Item = &'a Value> {
    report
        .probes
        .iter()
        .find(|probe| probe.name == probe_name && probe.state == "ok")
        .and_then(|probe: &ProbeReport| probe.detail.get(key))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
}
