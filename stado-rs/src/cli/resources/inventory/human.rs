//! The operator-facing tables: one per source, then the closing summary.

use serde_json::Value;

use super::model::ResourcesReport;
use crate::cli::reporting::table;

pub(super) fn print_human(report: &ResourcesReport) {
    table::print(
        &["ROLE", "LOCATOR", "STATE", "OBJECTS", "NEWEST", "ERROR"],
        &report
            .storage
            .data
            .as_array()
            .into_iter()
            .flatten()
            .map(|storage| {
                vec![
                    text(storage, "role"),
                    text(storage, "locator"),
                    text(storage, "state"),
                    number(storage, "object_count"),
                    text(storage, "newest_object_at"),
                    text(storage, "error"),
                ]
            })
            .collect::<Vec<Vec<String>>>(),
    );

    // A compute source without a provider list says so, rather than printing
    // an empty table that reads as a fleet with no providers.
    let providers: &[Value] = match report.compute.data["providers"].as_array() {
        Some(providers) => providers,
        None => {
            println!("compute: the compute source reported no provider list");
            &[]
        }
    };
    table::print(
        &[
            "PROVIDER",
            "CONFIG",
            "STATE",
            "INSTANCES",
            "ORPHANS",
            "OWNERSHIP",
            "ERROR",
        ],
        &providers
            .iter()
            .map(|provider| {
                vec![
                    text(provider, "provider"),
                    text(provider, "configured_state"),
                    text(provider, "state"),
                    number(provider, "instance_count"),
                    number(provider, "orphan_count"),
                    match provider
                        .get("ownership_authoritative")
                        .and_then(Value::as_bool)
                    {
                        Some(true) => "authoritative".to_string(),
                        Some(false) => "not authoritative".to_string(),
                        None => crate::cli::reporting::NOT_REPORTED.to_string(),
                    },
                    text(provider, "error"),
                ]
            })
            .collect::<Vec<Vec<String>>>(),
    );

    let instance_rows: Vec<Vec<String>> = providers
        .iter()
        .flat_map(|provider| {
            provider
                .get("instances")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .map(|instance| {
                    vec![
                        text(instance, "provider"),
                        text(instance, "reference"),
                        number(instance, "age_seconds"),
                        text(instance, "accelerator"),
                        listed(instance, "held_by"),
                    ]
                })
        })
        .collect();
    if !instance_rows.is_empty() {
        table::print(
            &["PROVIDER", "INSTANCE", "AGE S", "ACCELERATOR", "HELD BY"],
            &instance_rows,
        );
    }

    if let Some(probes) = report
        .gcp_inventory
        .data
        .get("probes")
        .and_then(Value::as_array)
    {
        table::print(
            &[
                "GCP PROBE",
                "SERVICE",
                "STATE",
                "COUNT",
                "RESOURCE",
                "ERROR",
            ],
            &probes
                .iter()
                .map(|probe| {
                    vec![
                        text(probe, "name"),
                        text(probe, "service"),
                        text(probe, "state"),
                        number(probe, "count"),
                        text(probe, "resource"),
                        text(probe, "error"),
                    ]
                })
                .collect::<Vec<Vec<String>>>(),
        );
    }

    let billing_rows: Vec<Vec<String>> = crate::capabilities::get("billing")
        .into_iter()
        .flat_map(|capability| capability.variants)
        .filter_map(|variant| {
            let provider = variant.provider?.as_str();
            let section = report.billing.data.get(provider)?;
            let metric = match variant.adapter {
                crate::capabilities::RuntimeAdapter::Billing(
                    crate::capabilities::BillingAdapter::Gcp,
                ) => "latest_month_net_usd",
                crate::capabilities::RuntimeAdapter::Billing(
                    crate::capabilities::BillingAdapter::Azure,
                ) => "available_balance",
                _ => return None,
            };
            Some(vec![
                provider.to_uppercase(),
                text(section, "status"),
                text(section, metric),
                text(section, "currency"),
                text(section, "detail"),
            ])
        })
        .collect();
    if !billing_rows.is_empty() {
        table::print(
            &["BILLING", "STATE", "COST / BALANCE", "CURRENCY", "DETAIL"],
            &billing_rows,
        );
    }

    let target_rows: Vec<Vec<String>> = report
        .host_registry
        .data
        .get("targets")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|target| {
            vec![
                text(target, "name"),
                text(target, "kind"),
                text(target, "gpu_type"),
                text(target, "release_platform"),
                text(target, "region"),
                listed(target, "hostnames"),
            ]
        })
        .collect();
    if !target_rows.is_empty() {
        table::print(
            &["TARGET", "KIND", "GPU", "PLATFORM", "REGION", "HOSTNAMES"],
            &target_rows,
        );
    }

    let targets = registry_count(report, "targets");
    let coordinators = registry_count(report, "coordinators");
    println!(
        "\nHosts: {targets}; coordinators: {coordinators}; billing: {}; storage objects: {}; visible VMs: {}; confirmed orphans: {}.",
        report.billing.state,
        report.summary.storage_objects,
        report.summary.visible_instances,
        report.summary.confirmed_orphan_instances,
    );
    for gap in &report.coverage_gaps {
        println!("COVERAGE GAP: {gap}");
    }
    println!(
        "Inventory state: {}; {} incomplete source(s); read-only.",
        report.summary.state, report.summary.incomplete_sources
    );
}

fn text(value: &Value, key: &str) -> String {
    match value.get(key) {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Null) | None => String::new(),
        Some(other) => other.to_string(),
    }
}

/// How many `key` entries the host registry source read, or why it cannot be
/// said: a registry that was not read is not a fleet of no hosts.
fn registry_count(report: &ResourcesReport, key: &str) -> String {
    match report.host_registry.data.get(key).and_then(Value::as_array) {
        Some(entries) => entries.len().to_string(),
        None => match &report.host_registry.error {
            Some(error) => format!("not read ({error})"),
            None => format!("not read (the host registry source carries no {key} list)"),
        },
    }
}

/// A count or age for a cell; one the row does not carry reads `not
/// reported`, since a blank count reads as none.
fn number(value: &Value, key: &str) -> String {
    crate::cli::reporting::field(&value[key])
}

/// A list field joined for a cell, or `not reported` when the row does not
/// carry it: an empty cell would read as a list nobody is on.
fn listed(value: &Value, key: &str) -> String {
    match value[key].as_array() {
        Some(items) => items
            .iter()
            .map(crate::cli::reporting::field)
            .collect::<Vec<_>>()
            .join(", "),
        None => crate::cli::reporting::NOT_REPORTED.to_string(),
    }
}
