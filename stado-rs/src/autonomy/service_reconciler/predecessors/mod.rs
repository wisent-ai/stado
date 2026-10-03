//! The units a product replaced, retired wherever that product now runs.
//!
//! A unit on a host that runs a catalog product's program under any label
//! but that product's one unit is work the product's single process took
//! over ([`service::predecessors_on_with`] finds them from what each unit
//! runs; no unit is named anywhere). Refusing to start them again is not
//! enough on a host where one is still loaded: it keeps running beside its
//! replacement, outside every release and review path. Each pass therefore
//! boots such a unit out and withdraws its autostart on every host that runs
//! the product replacing it, reading each host's units once, and then does
//! the same for [`strays`]: idle units in the fleet's namespace that no
//! product owns, reporting the live ones. Last, [`standby`] boots out every
//! standby unit the pass's sweep found serving.

use std::collections::{BTreeMap, BTreeSet};

use crate::autonomy::policy::{AutonomyMode, AutonomyPolicy};
use crate::deploy::service::{self, ServiceStatus, UndeclaredUnit, STATE_ACTIVE};
use crate::deploy::Runner;
use crate::queue::StorageError;
use crate::targets::ComputeTarget;

use super::gate::MutationGate;
use super::receipts::{ServiceReconcileOutcome, ServiceReconcileSummary};

mod listeners;
mod roles;
mod standby;
mod strays;

pub(super) use roles::{replaced, retake, taken_over};

/// A declared catalog service and its catalog entry: the replacements a pass
/// retires predecessors for, and asks about before repairing a unit.
pub(super) struct Replacement {
    pub(super) service: service::ManagedService,
    pub(super) entry: crate::deploy::service_catalog::CatalogService,
    /// Whether the pass found it running. A stopped one retires nothing; a
    /// listener handoff under way for it is undone.
    pub(super) active: bool,
}

/// The replacements among `statuses`, running or not: every declared
/// service that is a catalog product's one unit.
pub(super) fn replacements(statuses: &[ServiceStatus]) -> Vec<Replacement> {
    statuses
        .iter()
        .filter_map(|status| {
            let entry = crate::deploy::service_catalog::lookup(&status.service.name)
                .ok()
                .flatten()
                .filter(|entry| {
                    crate::deploy::service_catalog::owns_label(entry, status.service.unit_id())
                })?;
            Some(Replacement {
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

/// Retire each replacement's predecessors on its host, then every idle
/// undeclared fleet unit no product owns on the local hosts, then stop every
/// standby unit `findings` shows serving, through the pass's mutation gate.
/// Report mode and the emergency pause record the plan and touch nothing, as
/// for every repair. A role unit whose role the replacement's live process is
/// not proven to run is recorded `kept` and left running.
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
    let host_product = crate::deploy::service_catalog::host_process()
        .map(|entry| entry.name)
        .unwrap_or_default();
    // Each host's units are read once per pass, however many products it runs.
    let mut hosts: BTreeMap<String, Result<(ComputeTarget, Vec<UndeclaredUnit>), String>> =
        BTreeMap::new();
    for Replacement {
        service: running,
        entry,
        active,
    } in replacements
    {
        // Only the host Stado process hands listeners over, and a handoff
        // under way is judged even while that process is stopped.
        if !active && entry.name != host_product {
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
        if !hosts.contains_key(host) {
            let read = match crate::deploy::host_channel::canonical_target(host).await {
                Ok(target) => match service::loaded_units(&target, runner).await {
                    Ok(loaded) => Ok((target, loaded)),
                    Err(error) => Err(format!("its units could not be read: {error}")),
                },
                Err(error) => Err(error.to_string()),
            };
            hosts.insert(host.clone(), read);
        }
        let (target, loaded) = match &hosts[host] {
            Ok(read) => read,
            Err(error) => {
                summary.failures += 1;
                outcomes.push(row(
                    entry.name.as_str(),
                    "repair_failed",
                    false,
                    error.clone(),
                ));
                continue;
            }
        };
        let found = match service::predecessors_on_with(
            target,
            entry,
            active.then_some(running),
            loaded,
            runner,
        )
        .await
        {
            Ok(found) => found,
            Err(error) => {
                summary.failures += 1;
                outcomes.push(row(
                    entry.name.as_str(),
                    "repair_failed",
                    false,
                    error.to_string(),
                ));
                continue;
            }
        };
        if policy.mode == AutonomyMode::Report || policy.emergency_paused {
            for unit in found.units() {
                outcomes.push(row(
                    unit,
                    "planned",
                    false,
                    format!(
                        "{} runs its work; retirement not executed in this mode",
                        entry.name
                    ),
                ));
            }
            continue;
        }
        let listeners: Vec<_> = found
            .roles
            .iter()
            .filter(|role| service::listener_role(role))
            .cloned()
            .collect();
        if *active {
            for retirement in service::retire_found(target, running, found, runner).await {
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
            target,
            running,
            roles: &listeners,
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
