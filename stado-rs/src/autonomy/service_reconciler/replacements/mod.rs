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
use crate::deploy::service::{self, ServiceStatus};
use crate::deploy::service_catalog::CatalogService;
use crate::primitives::tally::Tally;
use crate::queue::StorageError;
use crate::deploy::Runner;
use crate::targets::ComputeTarget;

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

/// What `missing` found: the old units per `(host, catalog service)`, and
/// every unit whose catalog owner could not be read, with why.
type Missing = (
    BTreeMap<(String, String), Vec<String>>,
    Vec<(String, String, String)>,
);

/// `(host, catalog service)` → the old units declared there, for every host
/// that declares units running a catalog service's program under another
/// label but not that service. A host whose registry entry or stated home
/// cannot be read reports every unit it declares as unreadable with that
/// cause. A unit whose owner cannot be read is returned with the cause:
/// skipped, a replacement it needed was never planned and nothing said so.
async fn missing(statuses: &[ServiceStatus], runner: &Runner) -> Missing {
    let mut targets: BTreeMap<String, Result<(ComputeTarget, String), String>> = BTreeMap::new();
    let mut wanted: BTreeMap<(String, String), Vec<String>> = BTreeMap::new();
    let mut unreadable = Vec::new();
    for status in statuses {
        let host = &status.service.host;
        if !targets.contains_key(host) {
            targets.insert(host.clone(), stated_host(host, runner).await);
        }
        let (target, home) = match &targets[host] {
            Ok(stated) => stated,
            Err(error) => {
                unreadable.push((
                    host.clone(),
                    status.service.unit_id().to_string(),
                    error.clone(),
                ));
                continue;
            }
        };
        let entry = match service::declared_owner(target, &status.service, home) {
            Ok(Some(entry)) => entry,
            Ok(None) => continue,
            Err(error) => {
                unreadable.push((host.clone(), status.service.unit_id().to_string(), error));
                continue;
            }
        };
        let present = statuses
            .iter()
            .any(|other| &other.service.host == host && is_entry(other, &entry));
        if !present {
            wanted
                .entry((host.clone(), entry.name.clone()))
                .or_default()
                .push(status.service.unit_id().to_string());
        }
    }
    (wanted, unreadable)
}

/// `host`'s registry target and the home its own shell states, or the
/// sentence saying which of the two cannot be read.
async fn stated_host(host: &str, runner: &Runner) -> Result<(ComputeTarget, String), String> {
    let target = crate::deploy::host_channel::canonical_target(host)
        .await
        .map_err(|error| format!("the host cannot be resolved in the registry: {error}"))?;
    let home = crate::deploy::host_channel::remote_home(&target, runner)
        .await
        .map_err(|error| format!("the home the host states cannot be read: {error}"))?;
    Ok((target, home))
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
    runner: &Runner,
    gate: &mut MutationGate<'_>,
    summary: &mut ServiceReconcileSummary,
) -> Result<Vec<ServiceReconcileOutcome>, StorageError> {
    let catalog = match crate::deploy::service_catalog::all() {
        Ok(catalog) => catalog,
        Err(error) => {
            summary.failures.tally();
            return Ok(vec![row("-", "-", "-", "repair_failed", false, error)]);
        }
    };
    let mut outcomes = Vec::new();
    let (wanted, unreadable) = missing(statuses, runner).await;
    for (host, unit, error) in unreadable {
        summary.failures.tally();
        outcomes.push(row(
            &host,
            "-",
            &unit,
            "repair_failed",
            false,
            format!("the catalog service {unit} runs as on {host} cannot be read: {error}"),
        ));
    }
    for ((host, name), old) in wanted {
        // The catalog names the unit its product runs as; a product declared
        // without one is addressed by its name.
        let unit = match catalog
            .iter()
            .find(|entry| entry.name == name)
            .and_then(|entry| entry.unit.clone())
        {
            Some(unit) => unit,
            None => name.clone(),
        };
        let reason = format!(
            "{host} runs {} without {unit}, the one {name} process that replaces them",
            old.join(", ")
        );
        summary.planned.tally();
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
                summary.blocked.tally();
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
            unset_env: &[],
            reason: &reason,
            as_daemon: false,
            as_launch_agent: false,
            as_json: true,
        })
        .await
        .map_err(|error| error.to_string());
        match gate.release(&subject, &lease, ensured).await {
            Ok(receipt) => {
                summary.changed.tally();
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
                summary.failures.tally();
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
