//! The read-only audit pass: every source is probed independently so that one
//! disabled API or unreachable store degrades a single row instead of hiding
//! the rest of the fleet. A source that could not answer is reported as
//! blocked, degraded, skipped or unsupported and counted as incomplete.

mod configuration;
mod fields;
mod gcp;
mod summary;

use std::collections::BTreeSet;

use chrono::{SecondsFormat, Utc};
use serde_json::json;

use crate::cli::{blast_radius, instances, CmdError};
use crate::config;
use crate::providers::gcp::inventory as gcp_inventory;
use crate::queue::copy::Endpoint;
use crate::queue::JobStorage;

use super::{AuditArgs, ConfigurationSnapshot, RationalizationReport, SourceReport};
use configuration::{configuration_findings, orphan_instance_findings};
use gcp::gcp_findings;
use summary::{severity_rank, summarize};

pub(super) async fn build_report(args: &AuditArgs) -> Result<RationalizationReport, CmdError> {
    let primary = Endpoint::configured_primary();
    let backup = Endpoint::configured_backup();
    let active = config::wc_providers().to_vec();
    let disabled = config::wc_disabled_providers().to_vec();
    let configured: BTreeSet<String> = active.iter().chain(&disabled).cloned().collect();
    let now = Utc::now();

    let mut findings = configuration_findings(&primary, backup.as_ref(), &active, &disabled);
    let mut sources = vec![SourceReport {
        name: "stado-configuration".to_string(),
        state: "ok",
        detail: json!({
            "active_compute": active,
            "disabled_compute": disabled,
            "primary_storage": primary.describe(),
            "backup_storage": backup.as_ref().map(Endpoint::describe),
        }),
    }];

    let fleet_providers: Vec<String> =
        crate::capabilities::provider_ids(crate::capabilities::RuntimeFacet::Inventory)
            .into_iter()
            .map(|provider| provider.as_str().to_string())
            .filter(|provider| configured.contains(provider))
            .collect();
    if fleet_providers.is_empty() {
        sources.push(SourceReport {
            name: "agent-vm-ownership".to_string(),
            state: "skipped",
            detail: json!({"reason": "no enumerable cloud or marketplace compute provider is configured"}),
        });
    } else if primary.adapter() == Some(crate::capabilities::StorageAdapter::Local) {
        sources.push(SourceReport {
            name: "agent-vm-ownership".to_string(),
            state: "blocked",
            detail: json!({
                "reason": "device-local storage is not an authoritative ownership view for remote cloud agents",
                "remedy": "migrate the active queue to GCS, S3, or Azure Blob before using orphan VM recommendations",
            }),
        });
    } else {
        match JobStorage::new().await {
            Ok(store) => match instances::audit_inventory(&store, &fleet_providers).await {
                Ok(fleet) => {
                    for provider in &fleet_providers {
                        if let Some(error) = fleet.errors.get(provider) {
                            sources.push(SourceReport {
                                name: format!("{provider}-agent-vm-ownership"),
                                state: "blocked",
                                detail: json!({"error": error}),
                            });
                        } else {
                            let count = fleet
                                .rows
                                .iter()
                                .filter(|row| row.provider == *provider)
                                .count();
                            sources.push(SourceReport {
                                name: format!("{provider}-agent-vm-ownership"),
                                state: "ok",
                                detail: json!({"instances": count}),
                            });
                        }
                    }
                    findings.extend(orphan_instance_findings(&fleet.rows, args.min_age));
                }
                Err(error) => sources.push(SourceReport {
                    name: "agent-vm-ownership".to_string(),
                    state: "blocked",
                    detail: json!({"error": error.to_string()}),
                }),
            },
            Err(error) => sources.push(SourceReport {
                name: "agent-vm-ownership".to_string(),
                state: "blocked",
                detail: json!({
                    "error": error.to_string(),
                    "reason": "the authoritative queue and lease store could not be opened",
                }),
            }),
        }
    }

    for variant in crate::capabilities::get("inventory")
        .into_iter()
        .flat_map(|capability| capability.variants)
    {
        let Some(provider) = variant.provider else {
            continue;
        };
        let provider_configured = configured.iter().any(|name| provider.matches(name));
        match variant.adapter {
            crate::capabilities::RuntimeAdapter::Inventory(
                crate::capabilities::InventoryAdapter::Gcp,
            ) if provider_configured => {
                let options = blast_radius::gcp_inventory_options(&primary, backup.as_ref());
                let report = gcp_inventory::inspect(options).await;
                sources.push(SourceReport {
                    name: format!("{}-resource-inventory", variant.id),
                    state: if report.summary.state == "ok" {
                        "ok"
                    } else {
                        "degraded"
                    },
                    detail: json!({
                        "project": report.project,
                        "summary": report.summary,
                    }),
                });
                findings.extend(gcp_findings(
                    &report,
                    args.min_age,
                    now,
                    disabled.iter().any(|name| provider.matches(name)),
                ));
            }
            crate::capabilities::RuntimeAdapter::Inventory(
                crate::capabilities::InventoryAdapter::Gcp,
            ) => sources.push(SourceReport {
                name: format!("{}-resource-inventory", variant.id),
                state: "skipped",
                detail: json!({"reason": format!("{} is absent from providers and providers_disabled", provider)}),
            }),
            crate::capabilities::RuntimeAdapter::Inventory(_) if provider_configured => {
                sources.push(SourceReport {
                    name: format!("{}-resource-inventory", variant.id),
                    state: "unsupported",
                    detail: json!({
                        "reason": provider.inventory_limitation().unwrap_or(variant.summary),
                        "remedy": format!("review the {provider} provider console before accepting this report as complete"),
                    }),
                });
            }
            _ => {}
        }
    }

    findings.sort_by(|left, right| {
        severity_rank(left.severity)
            .cmp(&severity_rank(right.severity))
            .then_with(|| left.provider.cmp(&right.provider))
            .then_with(|| left.resource.cmp(&right.resource))
    });
    let incomplete_sources = sources
        .iter()
        .filter(|source| matches!(source.state, "blocked" | "degraded" | "unsupported"))
        .count();
    let summary = summarize(&findings, incomplete_sources);
    let report = RationalizationReport {
        schema_version: u8::from(true),
        generated_at: now.to_rfc3339_opts(SecondsFormat::Secs, true),
        read_only: true,
        min_age_seconds: args.min_age,
        configuration: ConfigurationSnapshot {
            active_compute: active,
            disabled_compute: disabled,
            primary_storage: primary.describe(),
            backup_storage: backup.as_ref().map(Endpoint::describe),
        },
        summary,
        sources,
        findings,
    };

    Ok(report)
}
