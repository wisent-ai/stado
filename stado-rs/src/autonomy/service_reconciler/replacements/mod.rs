//! The one product unit a host lacks while it still runs that product's old
//! units.
//!
//! A product's one process retires the units it replaced when it starts, and
//! the predecessor step retires them on every host whose registry declares
//! that process. A host whose registry declares only the old units runs
//! neither, so the old units would run there for ever. Each pass therefore
//! ensures the catalog service on such a host, through the pass's mutation
//! gate; the next pass finds it declared and retires the old units.

use std::collections::BTreeMap;

use crate::autonomy::policy::{AutonomyMode, AutonomyPolicy};
use crate::deploy::service::ServiceStatus;
use crate::deploy::service_catalog::CatalogService;
use crate::queue::StorageError;

use super::gate::MutationGate;
use super::receipts::{ServiceReconcileOutcome, ServiceReconcileSummary};

/// The action every row of this step records.
const ACTION: &str = "ensure_replacement";

/// Whether `status` is `entry`'s own service on its host.
fn is_entry(status: &ServiceStatus, entry: &CatalogService) -> bool {
    status.service.name == entry.name
        || entry
            .unit
            .as_deref()
            .is_some_and(|unit| status.service.unit_id() == unit)
}

/// Whether `status` is a unit `entry`'s one process replaced.
fn replaced_by(status: &ServiceStatus, entry: &CatalogService) -> bool {
    let unit = status.service.unit_id();
    let name = status.service.name.as_str();
    entry
        .retired_units
        .iter()
        .any(|retired| retired == unit || retired == name)
        || entry
            .role_units
            .iter()
            .any(|role| role.unit == unit || role.unit == name)
}

/// `(host, catalog service)` → the old units declared there, for every host
/// that declares units a catalog service replaced but not that service.
fn missing(
    statuses: &[ServiceStatus],
    catalog: &[CatalogService],
) -> BTreeMap<(String, String), Vec<String>> {
    let mut wanted: BTreeMap<(String, String), Vec<String>> = BTreeMap::new();
    for entry in catalog {
        for status in statuses.iter().filter(|status| replaced_by(status, entry)) {
            let host = &status.service.host;
            let present = statuses
                .iter()
                .any(|other| &other.service.host == host && is_entry(other, entry));
            if !present {
                wanted
                    .entry((host.clone(), entry.name.clone()))
                    .or_default()
                    .push(status.service.unit_id().to_string());
            }
        }
    }
    wanted
}

/// One row of this step.
fn row(
    host: &str,
    service: &str,
    unit: &str,
    classification: &str,
    changed: bool,
    detail: String,
) -> ServiceReconcileOutcome {
    ServiceReconcileOutcome {
        host: host.to_string(),
        service: service.to_string(),
        unit: unit.to_string(),
        beacon_state: "not-used".to_string(),
        endpoint_state: "not-used".to_string(),
        classification: classification.to_string(),
        action: ACTION.to_string(),
        changed,
        detail,
    }
}

/// Ensure the catalog service on every host that still runs only the units it
/// replaced. Report mode and the emergency pause record the plan and change
/// nothing; a refused or failed ensure is the row's failure with its own
/// sentence.
pub(super) async fn ensure_replacements(
    statuses: &[ServiceStatus],
    policy: &AutonomyPolicy,
    gate: &mut MutationGate<'_>,
    summary: &mut ServiceReconcileSummary,
) -> Result<Vec<ServiceReconcileOutcome>, StorageError> {
    let catalog = match crate::deploy::service_catalog::all() {
        Ok(catalog) => catalog,
        Err(error) => {
            summary.failures += 1;
            return Ok(vec![row("-", "-", "-", "repair_failed", false, error)]);
        }
    };
    let mut outcomes = Vec::new();
    for ((host, name), old) in missing(statuses, &catalog) {
        let unit = catalog
            .iter()
            .find(|entry| entry.name == name)
            .and_then(|entry| entry.unit.clone())
            .unwrap_or_else(|| name.clone());
        let reason = format!(
            "{host} runs {} without {unit}, the one {name} process that replaces them",
            old.join(", ")
        );
        summary.planned += 1;
        if policy.mode == AutonomyMode::Report || policy.emergency_paused {
            outcomes.push(row(
                &host,
                &name,
                &unit,
                "planned",
                false,
                format!("{reason}; ensure not executed in this mode"),
            ));
            continue;
        }
        let (subject, lease) = match gate.admit(&host, &unit).await? {
            Ok(admitted) => admitted,
            Err(refusal) => {
                summary.blocked += 1;
                outcomes.push(row(
                    &host,
                    &name,
                    &unit,
                    refusal.classification,
                    false,
                    refusal.detail,
                ));
                continue;
            }
        };
        let ensured = crate::cli::service::ensure_unit(crate::cli::service::EnsureOptions {
            name: &name,
            host: &host,
            from: None,
            args: &[],
            env: &[],
            reason: &reason,
            as_daemon: false,
            as_launch_agent: false,
            as_json: true,
        })
        .await
        .map_err(|error| error.to_string());
        match gate.release(&subject, &lease, ensured).await {
            Ok(receipt) => {
                summary.changed += 1;
                gate.record(None).await?;
                outcomes.push(row(
                    &host,
                    &name,
                    &receipt.label,
                    "reconciled",
                    true,
                    format!("{reason}; {} {}", receipt.action, receipt.label),
                ));
            }
            Err(error) => {
                summary.failures += 1;
                gate.record(Some(&error)).await?;
                outcomes.push(row(
                    &host,
                    &name,
                    &unit,
                    "repair_failed",
                    false,
                    format!("{reason}; service ensure {name} --host {host} failed: {error}"),
                ));
            }
        }
    }
    Ok(outcomes)
}
