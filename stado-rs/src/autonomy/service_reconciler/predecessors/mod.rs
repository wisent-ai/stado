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

use crate::autonomy::policy::{AutonomyMode, AutonomyPolicy};
use crate::deploy::service::{self, ServiceStatus, STATE_ACTIVE};
use crate::deploy::Runner;
use crate::queue::StorageError;

use super::gate::MutationGate;
use super::receipts::{ServiceReconcileOutcome, ServiceReconcileSummary};

mod roles;
mod standby;
mod strays;

pub(super) use roles::taken_over;

/// A running catalog service and its catalog entry, for each one whose entry
/// names retired or role units: the replacements a pass retires predecessors
/// for, and asks about before repairing a role unit.
pub(super) type Replacement = (
    service::ManagedService,
    crate::deploy::service_catalog::CatalogService,
);

/// The replacements among `statuses`: one row per declared service that is
/// running and whose catalog entry names at least one retired or role unit.
pub(super) fn replacements(statuses: &[ServiceStatus]) -> Vec<Replacement> {
    statuses
        .iter()
        .filter(|status| status.state == STATE_ACTIVE)
        .filter_map(|status| {
            let entry = crate::deploy::service_catalog::lookup(&status.service.name)
                .ok()
                .flatten()?;
            (!entry.retired_units.is_empty() || !entry.role_units.is_empty())
                .then(|| (status.service.clone(), entry))
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
    findings: &[crate::cli::service_verify::Finding],
    policy: &AutonomyPolicy,
    runner: &Runner,
    gate: &mut MutationGate<'_>,
    summary: &mut ServiceReconcileSummary,
) -> Result<Vec<ServiceReconcileOutcome>, StorageError> {
    let mut outcomes = Vec::new();
    for (running, entry) in replacements {
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
        for retirement in
            service::retire_catalog_predecessors(&target, entry, running, runner).await
        {
            let (classification, changed) = match retirement.state.as_str() {
                "retired" => ("retired", true),
                "handed_over" => ("handed_over", true),
                "restored" => ("restored", true),
                "kept" => ("kept", false),
                "awaiting_resolver" => ("awaiting_resolver", false),
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
    outcomes.extend(strays::retire_strays(policy, runner, summary).await);
    outcomes.extend(standby::stop_serving_standbys(findings, policy, runner, gate, summary).await?);
    Ok(outcomes)
}
