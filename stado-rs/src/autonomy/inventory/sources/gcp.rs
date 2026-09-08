//! The GCP reader: the blast-radius probe report folded into records.
//!
//! [`collect_gcp`] runs the probe set the blast-radius options ask for and
//! turns every probe that [`gcp_probe_shape`] recognizes into records via
//! [`gcp_resource`]; a probe error becomes an upstream error and, when it
//! reads as a permission refusal, a missing permission.

use std::collections::BTreeSet;

use chrono::{DateTime, Utc};
use serde_json::{json, Value};

use crate::autonomy::inventory::values::{
    canonical_revision, collect_resource_references, object_strings, permission_error,
    region_from_zone, source_state, value_text,
};
use crate::autonomy::model::{InventorySource, ResourceRecord};
use crate::capabilities::ProviderId;
use crate::queue::copy::Endpoint;

pub(in crate::autonomy::inventory) async fn collect_gcp(
    observed_at: DateTime<Utc>,
) -> InventorySource {
    let primary = Endpoint::configured_primary();
    let backup = Endpoint::configured_backup();
    let options = crate::cli::blast_radius::gcp_inventory_options(&primary, backup.as_ref());
    let project = options.project.clone();
    let report = crate::providers::gcp::inventory::inspect(options).await;
    let mut resources = Vec::new();
    let mut coverage = BTreeSet::new();
    let mut missing_permissions = Vec::new();
    let mut errors = Vec::new();
    for probe in &report.probes {
        coverage.insert(probe.service.clone());
        if let Some(error) = &probe.error {
            errors.push(format!("{}: {error}", probe.name));
            if permission_error(error) {
                missing_permissions.push(probe.name.clone());
            }
        }
        let Some((kind, key)) = gcp_probe_shape(&probe.name) else {
            continue;
        };
        let Some(items) = probe.detail.get(key).and_then(Value::as_array) else {
            continue;
        };
        for item in items {
            resources.push(gcp_resource(&project, kind, item, observed_at, &probe.name));
        }
    }
    let state = source_state(&report.summary.state, &errors);
    InventorySource {
        provider: ProviderId::Gcp,
        account: project,
        state,
        observed_at: observed_at.to_rfc3339(),
        coverage,
        missing_permissions,
        upstream_error: (!errors.is_empty()).then(|| errors.join("; ")),
        resources,
    }
}

fn gcp_probe_shape(name: &str) -> Option<(&'static str, &'static str)> {
    match name {
        "compute_instances" => Some(("instance", "instances")),
        "compute_disks" => Some(("persistent_disk", "disks")),
        "managed_instance_groups" => Some(("managed_instance_group", "managed_instance_groups")),
        "compute_reservations" => Some(("reservation", "reservations")),
        "static_addresses" => Some(("public_ip", "addresses")),
        "service_accounts" => Some(("service_account", "service_accounts")),
        "cloud_builds" => Some(("build", "builds")),
        "cloud_scheduler" => Some(("schedule", "jobs")),
        "cloud_functions" => Some(("function", "functions")),
        "cloud_run_services" => Some(("service", "services")),
        _ => None,
    }
}

fn gcp_resource(
    project: &str,
    kind: &str,
    item: &Value,
    observed_at: DateTime<Utc>,
    probe: &str,
) -> ResourceRecord {
    let name = value_text(item, &["name", "id", "self_link", "selfLink"])
        .unwrap_or_else(|| "unknown".to_string());
    let native = value_text(item, &["self_link", "selfLink", "id"]).unwrap_or_else(|| name.clone());
    let mut resource = ResourceRecord::new(
        ProviderId::Gcp,
        project,
        kind,
        native,
        name.clone(),
        observed_at,
    );
    resource.zone = value_text(item, &["zone"]);
    resource.region = value_text(item, &["region"]).or_else(|| {
        resource
            .zone
            .as_deref()
            .and_then(region_from_zone)
            .map(str::to_string)
    });
    resource.state = value_text(item, &["status", "state", "terminal_state"])
        .unwrap_or_else(|| "unknown".to_string())
        .to_ascii_lowercase();
    resource.created_at = value_text(item, &["creation_timestamp", "create_time", "created_at"]);
    resource.labels = object_strings(item.get("labels"));
    if item
        .get("stado_managed")
        .and_then(Value::as_bool)
        .unwrap_or(false)
        || name.starts_with("wisent-")
        || name.starts_with("stado-")
    {
        resource
            .labels
            .insert("managed-by".to_string(), "stado".to_string());
    }
    collect_resource_references(item, &mut resource.dependencies);
    resource.source_revision =
        value_text(item, &["fingerprint", "etag"]).or_else(|| canonical_revision(item));
    resource.evidence = json!({"probe": probe, "item": item});
    resource.apply_identity_labels();
    resource
}
