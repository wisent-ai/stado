//! The units a product replaced, retired wherever that product now runs.
//!
//! The catalog's `retired_units` names the hand-made or superseded units one
//! product's single process took over. Refusing to start them again is not
//! enough on a host where one is still loaded: it keeps running beside its
//! replacement, outside every release and review path. Each pass therefore
//! boots such a unit out and withdraws its autostart on every host that runs
//! the product replacing it, and then does the same for [`strays`]: failing
//! units in the fleet's namespace that nothing declares at all. Last,
//! [`standby`] boots out every standby unit the pass's sweep found serving.

use std::collections::BTreeSet;

use crate::autonomy::policy::{AutonomyMode, AutonomyPolicy};
use crate::deploy::service::{self, ServiceStatus, STATE_ACTIVE};
use crate::deploy::Runner;
use crate::queue::StorageError;

use super::gate::MutationGate;
use super::receipts::{ServiceReconcileOutcome, ServiceReconcileSummary};

mod listeners;
mod roles;
mod standby;
mod strays;

pub(super) use roles::taken_over;

/// A declared catalog service and its catalog entry, for each one whose entry
/// names retired or role units: the replacements a pass retires predecessors
/// for, and asks about before repairing a role unit.
pub(super) struct Replacement {
    pub(super) service: service::ManagedService,
    pub(super) entry: crate::deploy::service_catalog::CatalogService,
    /// Whether the pass found it running. A stopped one retires nothing; a
    /// listener handoff under way for it is undone.
    pub(super) active: bool,
}

/// The replacements among `statuses`, running or not.
pub(super) fn replacements(statuses: &[ServiceStatus]) -> Vec<Replacement> {
    statuses
        .iter()
        .filter_map(|status| {
            let entry = crate::deploy::service_catalog::lookup(&status.service.name)
                .ok()
                .flatten()?;
            (!entry.retired_units.is_empty() || !entry.role_units.is_empty()).then(|| Replacement {
                service: status.service.clone(),
                entry,
                active: status.state == STATE_ACTIVE,
            })
        })
        .collect()
}

/// `(host, unit)` for every unit the registry declares, under its unit id
/// and its name: a handoff is started only for a unit the pass can repair.
pub(super) fn declared_units(statuses: &[ServiceStatus]) -> BTreeSet<(String, String)> {
    statuses
        .iter()
        .flat_map(|status| {
            let host = status.service.host.clone();
            [
                (host.clone(), status.service.unit_id().to_string()),
                (host, status.service.name.clone()),
            ]
        })
        .collect()
}

/// Retire each replacement's predecessors on its host, then every failing
/// undeclared fleet unit on the local hosts, then stop every standby unit
/// `findings` shows serving, through the pass's mutation gate. Report mode
/// and the emergency pause record the plan and touch nothing, as for every
/// repair. A role unit whose role the replacement's live process is not
/// proven to run is recorded `kept` and left running.
pub(super) async fn retire(
    replacements: &[Replacement],
    declared: &BTreeSet<(String, String)>,
    findings: &[crate::cli::service_verify::Finding],
    policy: &AutonomyPolicy,
    runner: &Runner,
    gate: &mut MutationGate<'_>,
    summary: &mut ServiceReconcileSummary,
) -> Result<Vec<ServiceReconcileOutcome>, StorageError> {
    let mut outcomes = Vec::new();
    for Replacement {
        service: running,
        entry,
        active,
    } in replacements
    {
        let handoffs = entry.role_units.iter().any(service::listener_role);
        if !active && !handoffs {
            continue;
        }
        let host = &running.host;
        let row = |unit: &str, classification: &str, changed: bool, detail: String| {
            ServiceReconcileOutcome {
                host: host.clone(),
                service: unit.to_string(),
                unit: unit.to_string(),
                beacon_state: "not-used".to_string(),
                endpoint_state: "not-used".to_string(),
                classification: classification.to_string(),
                action: "retire_predecessor".to_string(),
                changed,
                detail,
            }
        };
        let units: Vec<&String> = entry
            .retired_units
            .iter()
            .chain(entry.role_units.iter().map(|role| &role.unit))
            .collect();
        if policy.mode == AutonomyMode::Report || policy.emergency_paused {
            for unit in &units {
                outcomes.push(row(
                    unit,
                    "planned",
                    false,
                    format!(
                        "{} replaced it; retirement not executed in this mode",
                        entry.name
                    ),
                ));
            }
            continue;
        }
        let target = match crate::deploy::host_channel::canonical_target(host).await {
            Ok(target) => target,
            Err(error) => {
                summary.failures += 1;
                for unit in &units {
                    outcomes.push(row(unit, "repair_failed", false, error.to_string()));
                }
                continue;
            }
        };
        if *active {
            for retirement in
                service::retire_catalog_predecessors(&target, entry, running, runner).await
            {
                let (classification, changed) = match retirement.state.as_str() {
                    "retired" => ("retired", true),
                    "kept" => ("kept", false),
                    "absent" => continue,
                    _ => ("repair_failed", false),
                };
                if changed {
                    summary.changed += 1;
                } else if classification == "repair_failed" {
                    summary.failures += 1;
                }
                outcomes.push(row(
                    &retirement.unit,
                    classification,
                    changed,
                    format!("replaced by {}: {}", entry.name, retirement.detail),
                ));
            }
        }
        let replaced = listeners::Replaced {
            target: &target,
            running,
            entry,
            active: *active,
        };
        outcomes.extend(
            listeners::hand_over_listeners(&replaced, declared, runner, gate, summary).await?,
        );
    }
    outcomes.extend(strays::retire_strays(policy, runner, summary).await);
    outcomes.extend(standby::stop_serving_standbys(findings, policy, runner, gate, summary).await?);
    Ok(outcomes)
}
