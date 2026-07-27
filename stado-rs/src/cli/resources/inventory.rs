//! `stado resources show` — one read-only, provider-neutral inventory.
//!
//! Every source is fault-isolated. A cloud or credential failure is represented
//! as a degraded source instead of erasing the resources returned by the other
//! providers.

use std::collections::BTreeSet;

use chrono::{SecondsFormat, Utc};
use serde::Serialize;
use serde_json::{json, Value};

use super::ShowArgs;
use crate::cli::{blast_radius, instances, table, CmdError};
use crate::config;
use crate::monitor::billing;
use crate::providers::{gcp::inventory as gcp_inventory, get_provider};
use crate::queue::copy::Endpoint;
use crate::queue::JobStorage;

#[derive(Debug, Clone, Serialize)]
struct SourceReport {
    name: &'static str,
    state: String,
    data: Value,
    error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct ConfigurationReport {
    active_compute: Vec<String>,
    disabled_compute: Vec<String>,
    primary_storage: String,
    backup_storage: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct Summary {
    state: &'static str,
    configured_providers: usize,
    visible_instances: usize,
    confirmed_orphan_instances: usize,
    storage_objects: usize,
    incomplete_sources: usize,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct ResourcesReport {
    schema_version: u8,
    generated_at: String,
    read_only: bool,
    configuration: ConfigurationReport,
    summary: Summary,
    storage: SourceReport,
    compute: SourceReport,
    host_registry: SourceReport,
    gcp_inventory: SourceReport,
    billing: SourceReport,
    coverage_gaps: Vec<String>,
}

pub async fn run(args: &ShowArgs) -> Result<(), CmdError> {
    let report = build(args).await?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print_human(&report);
    }
    Ok(())
}

pub(crate) async fn build(args: &ShowArgs) -> Result<ResourcesReport, CmdError> {
    let primary = Endpoint::configured_primary();
    let backup = Endpoint::configured_backup();
    let active = config::wc_providers().to_vec();
    let disabled = config::wc_disabled_providers().to_vec();
    let configured: BTreeSet<String> = active.iter().chain(&disabled).cloned().collect();
    let mut enumerable: Vec<String> = ["gcp", "azure", "aws"]
        .into_iter()
        .filter(|provider| configured.contains(*provider))
        .map(str::to_string)
        .collect();
    if let Some(provider) = args.provider.as_deref() {
        if !enumerable.iter().any(|name| name == provider) {
            return Err(CmdError::usage(format!(
                "provider {provider:?} is not a configured enumerable cloud"
            )));
        }
        enumerable.retain(|name| name == provider);
    }

    let storage_future = inspect_storage(&primary, backup.as_ref());
    let compute_future = inspect_compute(&enumerable, &active, &disabled, &primary);
    let registry_future = inspect_registry();
    let gcp_future = inspect_gcp(configured.contains("gcp"), &primary, backup.as_ref());
    let billing_future = inspect_billing(&configured);
    let (storage, compute, host_registry, gcp_inventory, billing) = tokio::join!(
        storage_future,
        compute_future,
        registry_future,
        gcp_future,
        billing_future,
    );

    let coverage_gaps = coverage_gaps(&configured);
    let incomplete_sources = [&storage, &compute, &host_registry, &gcp_inventory, &billing]
        .into_iter()
        .filter(|source| !matches!(source.state.as_str(), "ok" | "skipped"))
        .count()
        .saturating_add(usize::from(!coverage_gaps.is_empty()));
    let visible_instances = compute
        .data
        .get("providers")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|provider| provider.get("instances").and_then(Value::as_array))
        .map(Vec::len)
        .sum();
    let confirmed_orphan_instances = compute
        .data
        .get("providers")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|provider| provider.get("orphan_count").and_then(Value::as_u64))
        .map(|count| count as usize)
        .sum();
    let storage_objects = storage
        .data
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|entry| entry.get("object_count").and_then(Value::as_u64))
        .map(|count| count as usize)
        .sum();
    let summary = Summary {
        state: if incomplete_sources == usize::default() {
            "complete"
        } else {
            "incomplete"
        },
        configured_providers: configured.len(),
        visible_instances,
        confirmed_orphan_instances,
        storage_objects,
        incomplete_sources,
    };
    let report = ResourcesReport {
        schema_version: u8::from(true),
        generated_at: Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true),
        read_only: true,
        configuration: ConfigurationReport {
            active_compute: active,
            disabled_compute: disabled,
            primary_storage: primary.describe(),
            backup_storage: backup.as_ref().map(Endpoint::describe),
        },
        summary,
        storage,
        compute,
        host_registry,
        gcp_inventory,
        billing,
        coverage_gaps,
    };

    Ok(report)
}

async fn inspect_storage(primary: &Endpoint, backup: Option<&Endpoint>) -> SourceReport {
    let (primary, backup) = tokio::join!(
        blast_radius::storage_resource_report("primary", Some(primary)),
        blast_radius::storage_resource_report("backup", backup),
    );
    let reports = vec![primary, backup];
    let state = if reports.iter().any(|report| {
        matches!(
            report.get("state").and_then(Value::as_str),
            Some("unreachable" | "degraded")
        )
    }) {
        "degraded"
    } else {
        "ok"
    };
    SourceReport {
        name: "queue-storage",
        state: state.to_string(),
        data: Value::Array(reports),
        error: None,
    }
}

async fn inspect_compute(
    enumerable: &[String],
    active: &[String],
    disabled: &[String],
    primary: &Endpoint,
) -> SourceReport {
    let mut reports = Vec::new();
    let mut source_errors = Vec::new();
    let authoritative = primary.kind != "local";

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
        let (state, reason) = match provider.as_str() {
            "local" => (
                "registry",
                "physical local hosts are represented in host_registry, not a cloud VM fleet",
            ),
            "box" => (
                "external",
                "box capacity is externally owned and has no standing Stado VM inventory",
            ),
            other => (
                "unsupported",
                if other.is_empty() {
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

async fn inspect_registry() -> SourceReport {
    match crate::targets::load_registry_auto().await {
        Ok(registry) => SourceReport {
            name: "host-registry",
            state: "ok".to_string(),
            data: json!({
                "target_count": registry.targets.len(),
                "coordinator_count": registry.coordinators.len(),
                "targets": registry.targets,
                "coordinators": registry.coordinators,
            }),
            error: None,
        },
        Err(error) => SourceReport {
            name: "host-registry",
            state: "blocked".to_string(),
            data: Value::Null,
            error: Some(error.to_string()),
        },
    }
}

async fn inspect_gcp(enabled: bool, primary: &Endpoint, backup: Option<&Endpoint>) -> SourceReport {
    if !enabled {
        return SourceReport {
            name: "gcp-inventory",
            state: "skipped".to_string(),
            data: Value::Null,
            error: None,
        };
    }
    let options = blast_radius::gcp_inventory_options(primary, backup);
    let report = gcp_inventory::inspect(options).await;
    let state = report.summary.state.clone();
    SourceReport {
        name: "gcp-inventory",
        state,
        data: serde_json::to_value(report).expect("GCP inventory serialization is infallible"),
        error: None,
    }
}

async fn inspect_billing(configured: &BTreeSet<String>) -> SourceReport {
    let billed: Vec<&str> = ["gcp", "azure"]
        .into_iter()
        .filter(|provider| configured.contains(*provider))
        .collect();
    if billed.is_empty() {
        return SourceReport {
            name: "billing",
            state: "skipped".to_string(),
            data: Value::Null,
            error: None,
        };
    }
    let store = match JobStorage::new().await {
        Ok(store) => store,
        Err(error) => {
            return SourceReport {
                name: "billing",
                state: "blocked".to_string(),
                data: Value::Null,
                error: Some(error.to_string()),
            }
        }
    };
    match tokio::time::timeout(crate::doctor::PROBE_TIMEOUT, billing::live_snapshot(&store)).await {
        Ok(snapshot) => {
            let failures: Vec<String> = billed
                .iter()
                .filter_map(|provider| {
                    let status = snapshot
                        .get(*provider)
                        .and_then(|section| section.get("status"))
                        .and_then(Value::as_str)
                        .unwrap_or("unknown");
                    (status != "ok").then(|| format!("{provider}: {status}"))
                })
                .collect();
            SourceReport {
                name: "billing",
                state: if failures.is_empty() {
                    "ok".to_string()
                } else {
                    "degraded".to_string()
                },
                data: snapshot,
                error: if failures.is_empty() {
                    None
                } else {
                    Some(failures.join("; "))
                },
            }
        }
        Err(_) => SourceReport {
            name: "billing",
            state: "blocked".to_string(),
            data: Value::Null,
            error: Some(format!(
                "billing inventory exceeded {:?}",
                crate::doctor::PROBE_TIMEOUT
            )),
        },
    }
}

fn coverage_gaps(configured: &BTreeSet<String>) -> Vec<String> {
    let mut gaps = Vec::new();
    if configured.contains("aws") {
        gaps.push(
            "AWS: agent VM inventory is complete; EBS, Elastic IP, reservations, non-Stado resources, and AWS cost data are not enumerated"
                .to_string(),
        );
    }
    if configured.contains("azure") {
        gaps.push(
            "Azure: agent VM inventory is complete; managed disks, public IPs, reservations and non-Stado resources are not enumerated"
                .to_string(),
        );
    }
    if configured.contains("box") {
        gaps.push(
            "Box: externally owned marketplace capacity has no standing VM inventory".to_string(),
        );
    }
    gaps
}

fn print_human(report: &ResourcesReport) {
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
            .collect(),
    );

    let providers: Vec<Value> = report
        .compute
        .data
        .get("providers")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
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
                    if provider
                        .get("ownership_authoritative")
                        .and_then(Value::as_bool)
                        .unwrap_or(false)
                    {
                        "authoritative".to_string()
                    } else {
                        "unknown".to_string()
                    },
                    text(provider, "error"),
                ]
            })
            .collect(),
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
                        instance
                            .get("held_by")
                            .and_then(Value::as_array)
                            .map(|items| {
                                items
                                    .iter()
                                    .filter_map(Value::as_str)
                                    .collect::<Vec<_>>()
                                    .join(", ")
                            })
                            .unwrap_or_default(),
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
                .collect(),
        );
    }

    let billing_rows: Vec<Vec<String>> = ["gcp", "azure"]
        .into_iter()
        .filter_map(|provider| {
            let section = report.billing.data.get(provider)?;
            Some(vec![
                provider.to_uppercase(),
                text(section, "status"),
                if provider == "gcp" {
                    text(section, "latest_month_net_usd")
                } else {
                    text(section, "available_balance")
                },
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
                number(target, "slots"),
                text(target, "region"),
                target
                    .get("hostnames")
                    .and_then(Value::as_array)
                    .map(|items| {
                        items
                            .iter()
                            .filter_map(Value::as_str)
                            .collect::<Vec<_>>()
                            .join(", ")
                    })
                    .unwrap_or_default(),
            ]
        })
        .collect();
    if !target_rows.is_empty() {
        table::print(
            &["TARGET", "KIND", "GPU", "SLOTS", "REGION", "HOSTNAMES"],
            &target_rows,
        );
    }

    let targets = report
        .host_registry
        .data
        .get("targets")
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or_default();
    let coordinators = report
        .host_registry
        .data
        .get("coordinators")
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or_default();
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

fn number(value: &Value, key: &str) -> String {
    match value.get(key) {
        Some(Value::Number(number)) => number.to_string(),
        _ => String::new(),
    }
}
