//! The units a product replaced, retired wherever that product now runs.
//!
//! The catalog's `retired_units` names the hand-made or superseded units one
//! product's single process took over. Refusing to start them again is not
//! enough on a host where one is still loaded: it keeps running beside its
//! replacement, outside every release and review path. Each pass therefore
//! boots such a unit out and withdraws its autostart on every host that runs
//! the product replacing it.

use crate::autonomy::policy::{AutonomyMode, AutonomyPolicy};
use crate::deploy::service::{self, ServiceStatus, STATE_ACTIVE};
use crate::deploy::Runner;

use super::receipts::{ServiceReconcileOutcome, ServiceReconcileSummary};

/// The hosts and catalog entries whose predecessors a pass must retire: one
/// row per declared service that is running and whose catalog entry names at
/// least one retired unit.
pub(super) fn replacements(
    statuses: &[ServiceStatus],
) -> Vec<(String, crate::deploy::service_catalog::CatalogService)> {
    statuses
        .iter()
        .filter(|status| status.state == STATE_ACTIVE)
        .filter_map(|status| {
            let entry = crate::deploy::service_catalog::lookup(&status.service.name)
                .ok()
                .flatten()?;
            (!entry.retired_units.is_empty()).then(|| (status.service.host.clone(), entry))
        })
        .collect()
}

/// Retire each replacement's predecessors on its host. Report mode and the
/// emergency pause record the plan and touch nothing, as for every repair.
pub(super) async fn retire(
    replacements: &[(String, crate::deploy::service_catalog::CatalogService)],
    policy: &AutonomyPolicy,
    runner: &Runner,
    summary: &mut ServiceReconcileSummary,
) -> Vec<ServiceReconcileOutcome> {
    let mut outcomes = Vec::new();
    for (host, entry) in replacements {
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
        if policy.mode == AutonomyMode::Report || policy.emergency_paused {
            for unit in &entry.retired_units {
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
                for unit in &entry.retired_units {
                    outcomes.push(row(unit, "repair_failed", false, error.to_string()));
                }
                continue;
            }
        };
        for retirement in service::retire_catalog_predecessors(&target, entry, runner).await {
            let (classification, changed) = match retirement.state.as_str() {
                "retired" => ("retired", true),
                "absent" => continue,
                _ => ("repair_failed", false),
            };
            if changed {
                summary.changed += 1;
            } else {
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
    outcomes
}
