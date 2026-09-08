//! The cloud VM fleet: ownership-authoritative when the queue can be read,
//! degraded to a direct provider listing when it cannot.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::cli::instances;
use crate::cli::resources::inventory::model::SourceReport;
use crate::providers::get_provider;
use crate::queue::copy::Endpoint;
use crate::queue::JobStorage;

pub(in crate::cli::resources::inventory) async fn inspect_compute(
    enumerable: &[String],
    active: &[String],
    disabled: &[String],
    primary: &Endpoint,
) -> SourceReport {
    let mut reports = Vec::new();
    let mut source_errors = Vec::new();
    let authoritative = primary.adapter() != Some(crate::capabilities::StorageAdapter::Local);

    if !enumerable.is_empty() && authoritative {
        match JobStorage::new().await {
            Ok(store) => match instances::audit_inventory(&store, enumerable).await {
                Ok(fleet) => {
                    for provider in enumerable {
                        if let Some(error) = fleet.errors.get(provider) {
                            source_errors.push(format!("{provider}: {error}"));
                            reports.push(provider_error(provider, active, error));
                            continue;
                        }
                        let rows: Vec<&instances::AuditInstanceRow> = fleet
                            .rows
                            .iter()
                            .filter(|row| row.provider == *provider)
                            .collect();
                        let orphan_count = rows.iter().filter(|row| row.is_orphan()).count();
                        let instances: Vec<Value> = rows
                            .iter()
                            .map(|row| {
                                json!({
                                    "reference": row.reference,
                                    "provider": row.provider,
                                    "age_seconds": row.age_seconds,
                                    "accelerator": row.accel,
                                    "held_by": row.held_by,
                                    "ownership": if row.is_orphan() { "orphan" } else { "held" },
                                })
                            })
                            .collect();
                        reports.push(json!({
                            "provider": provider,
                            "configured_state": configured_state(provider, active, disabled),
                            "state": "ok",
                            "ownership_authoritative": true,
                            "instance_count": instances.len(),
                            "orphan_count": orphan_count,
                            "instances": instances,
                            "error": null,
                        }));
                    }
                }
                Err(error) => {
                    source_errors.push(error.to_string());
                    reports.extend(
                        direct_compute_reports(
                            enumerable,
                            active,
                            disabled,
                            Some(error.to_string()),
                        )
                        .await,
                    );
                }
            },
            Err(error) => {
                source_errors.push(error.to_string());
                reports.extend(
                    direct_compute_reports(enumerable, active, disabled, Some(error.to_string()))
                        .await,
                );
            }
        }
    } else if !enumerable.is_empty() {
        let reason =
            "device-local queue storage cannot authoritatively resolve remote VM ownership";
        source_errors.push(reason.to_string());
        reports.extend(
            direct_compute_reports(enumerable, active, disabled, Some(reason.to_string())).await,
        );
    }

    let configured_providers: BTreeSet<&String> = active.iter().chain(disabled).collect();
    for provider in configured_providers {
        if enumerable.contains(provider) {
            continue;
        }
        let adapter =
            crate::capabilities::variant(crate::capabilities::RuntimeFacet::Compute, provider)
                .map(|variant| variant.adapter);
        let (state, reason) = match adapter {
            Some(crate::capabilities::RuntimeAdapter::Compute(
                crate::capabilities::ComputeAdapter::ExistingHost,
            )) => (
                "registry",
                "physical local hosts are represented in host_registry, not a cloud VM fleet",
            ),
            Some(crate::capabilities::RuntimeAdapter::Compute(
                crate::capabilities::ComputeAdapter::Box
                | crate::capabilities::ComputeAdapter::VastHost,
            )) => (
                "external",
                "externally owned capacity has no standing Stado VM inventory",
            ),
            _ => (
                "unsupported",
                if provider.is_empty() {
                    "empty provider name"
                } else {
                    "this compute variant has no provider-neutral resource enumerator"
                },
            ),
        };
        reports.push(json!({
            "provider": provider,
            "configured_state": configured_state(provider, active, disabled),
            "state": state,
            "ownership_authoritative": false,
            "instance_count": null,
            "orphan_count": null,
            "instances": [],
            "error": reason,
        }));
    }

    let incomplete = !source_errors.is_empty()
        || reports.iter().any(|report| {
            matches!(
                report.get("state").and_then(Value::as_str),
                Some("blocked" | "degraded" | "unsupported")
            )
        });
    SourceReport {
        name: "compute",
        state: if incomplete {
            "degraded".to_string()
        } else {
            "ok".to_string()
        },
        data: json!({"providers": reports}),
        error: if source_errors.is_empty() {
            None
        } else {
            Some(source_errors.join("; "))
        },
    }
}

async fn direct_compute_reports(
    providers: &[String],
    active: &[String],
    disabled: &[String],
    ownership_error: Option<String>,
) -> Vec<Value> {
    let mut reports = Vec::new();
    for provider in providers {
        let client = match get_provider(provider) {
            Ok(client) => client,
            Err(error) => {
                reports.push(provider_error(provider, active, &error.to_string()));
                continue;
            }
        };
        match client.list_running_instance_refs_with_age().await {
            Ok(rows) => {
                let instances: Vec<Value> = rows
                    .into_iter()
                    .map(|(reference, age_seconds)| {
                        json!({
                            "reference": reference,
                            "provider": provider,
                            "age_seconds": age_seconds,
                            "accelerator": null,
                            "held_by": [],
                            "ownership": "unknown",
                        })
                    })
                    .collect();
                reports.push(json!({
                    "provider": provider,
                    "configured_state": configured_state(provider, active, disabled),
                    "state": "degraded",
                    "ownership_authoritative": false,
                    "instance_count": instances.len(),
                    "orphan_count": null,
                    "instances": instances,
                    "error": ownership_error,
                }));
            }
            Err(error) => reports.push(provider_error(provider, active, &error.to_string())),
        }
    }
    reports
}

fn provider_error(provider: &str, active: &[String], error: &str) -> Value {
    json!({
        "provider": provider,
        "configured_state": if active.iter().any(|name| name == provider) { "active" } else { "disabled" },
        "state": "blocked",
        "ownership_authoritative": false,
        "instance_count": null,
        "orphan_count": null,
        "instances": [],
        "error": error,
    })
}

fn configured_state(provider: &str, active: &[String], disabled: &[String]) -> &'static str {
    if active.iter().any(|name| name == provider) {
        "active"
    } else if disabled.iter().any(|name| name == provider) {
        "disabled"
    } else {
        "unknown"
    }
}
