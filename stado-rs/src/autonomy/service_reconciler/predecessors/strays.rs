//! Units in the fleet's own label namespace that the registry never declared
//! and no catalog product owns, on every local host.
//!
//! `service list --undeclared` names them, and nothing more: a stray copy of
//! a product bootstrapped by hand under a doubled label, running a program
//! from a loose folder in the home directory, can exit 1 on every start and
//! be started again at every login while no pass ever acts on the row. A unit
//! that runs a catalog product's program belongs to that product and is
//! retired with its other predecessors; one that runs no product's program
//! has no owner that will repair it.
//!
//! Such a unit that runs nothing is retired: booted out and its autostart
//! withdrawn, the unit file left in place, when it last exited non-zero or
//! when no launchd domain holds it any more, so its file would only start it
//! again at the next login. One that is loaded and live, or loaded and idle
//! between scheduled runs, may be serving something: it is never stopped by
//! this pass and is reported as `undeclared_live`, naming its program, until
//! it becomes a catalog product's one unit or is removed. A program under
//! `~/.stado/` is left alone even when it runs nothing, because Stado installs
//! those itself and one may be ahead of the registry document for a pass.

use crate::autonomy::policy::{AutonomyMode, AutonomyPolicy};
use crate::deploy::service::{self, UndeclaredUnit};
use crate::deploy::Runner;

use super::super::receipts::{ServiceReconcileOutcome, ServiceReconcileSummary};

/// The path segment of the directory Stado installs its own programs into.
const STADO_HOME_SEGMENT: &str = "/.stado/";

/// Is `unit` an undeclared fleet unit that runs nothing and either failed or
/// is held by no launchd domain?
fn is_idle_stray(unit: &UndeclaredUnit) -> bool {
    let exited_nonzero = unit
        .last_exit
        .or_else(|| unit.status.trim().parse::<i64>().ok())
        .is_some_and(|code| code != 0);
    unit.classification() == "undeclared"
        && unit.pid.trim().is_empty()
        && (exited_nonzero || unit.loaded_domains.is_empty())
        && !unit.declared_program().contains(STADO_HOME_SEGMENT)
}

/// The catalog product whose program `unit` runs on `target`, when one does.
fn owner(
    target: &crate::targets::ComputeTarget,
    unit: &UndeclaredUnit,
) -> Result<Option<String>, String> {
    Ok(crate::deploy::service_catalog::owner_of(
        &unit.label,
        &unit.declared_program(),
        &unit.running_program,
        &crate::deploy::service_catalog::home_for(target),
        &target.release_platform,
        &target.name,
    )?
    .map(|entry| entry.name))
}

fn row(
    unit: &UndeclaredUnit,
    classification: &str,
    changed: bool,
    detail: String,
) -> ServiceReconcileOutcome {
    ServiceReconcileOutcome {
        host: unit.host.clone(),
        service: unit.label.clone(),
        unit: unit.label.clone(),
        beacon_state: "not-used".to_string(),
        endpoint_state: "not-used".to_string(),
        classification: classification.to_string(),
        action: "retire_stray".to_string(),
        changed,
        detail,
    }
}

/// Retire every idle undeclared fleet unit no product owns on each local
/// host, and report each live one. Report mode and the emergency pause record
/// the plan and touch nothing.
pub(in crate::autonomy::service_reconciler) async fn retire_strays(
    policy: &AutonomyPolicy,
    runner: &Runner,
    summary: &mut ServiceReconcileSummary,
) -> Vec<ServiceReconcileOutcome> {
    let mut outcomes = Vec::new();
    let registry = match crate::targets::fetch_registry_or_last_good().await {
        Ok((registry, _)) => registry,
        Err(error) => {
            summary.failures += 1;
            let detail = format!("registry unreadable, no host scanned: {error}");
            scan_failed(&mut outcomes, "", &detail);
            return outcomes;
        }
    };
    let plan_only = policy.mode == AutonomyMode::Report || policy.emergency_paused;
    for target in registry.local_targets() {
        let units = match service::undeclared_units(target, runner).await {
            Ok(units) => units,
            Err(error) => {
                summary.failures += 1;
                let detail = format!("undeclared-unit scan failed: {error}");
                scan_failed(&mut outcomes, &target.name, &detail);
                continue;
            }
        };
        for unit in units
            .iter()
            .filter(|unit| unit.classification() == "undeclared")
        {
            // A product's own unit is declared by the catalog whatever the
            // registry says, and ensure and the repair steps own it.
            if crate::deploy::service_catalog::is_catalog_unit(&unit.label).unwrap_or(true) {
                continue;
            }
            match owner(target, unit) {
                Ok(None) => {}
                // Its product's predecessor step retires it.
                Ok(Some(_)) => continue,
                Err(error) => {
                    summary.failures += 1;
                    outcomes.push(row(unit, "repair_failed", false, error));
                    continue;
                }
            }
            let program = unit.declared_program();
            if !is_idle_stray(unit) {
                if !unit.pid.trim().is_empty() || !unit.loaded_domains.is_empty() {
                    outcomes.push(row(
                        unit,
                        "undeclared_live",
                        false,
                        format!(
                            "undeclared, loaded{}, program {program}: no catalog product runs \
                             it, so it must become a product's one unit or be removed",
                            if unit.pid.trim().is_empty() {
                                String::new()
                            } else {
                                format!(" as pid {}", unit.pid.trim())
                            }
                        ),
                    ));
                }
                continue;
            }
            let why = format!(
                "undeclared, not running, last exit {}, held by {}, program {program}",
                unit.status,
                if unit.loaded_domains.is_empty() {
                    "no launchd domain".to_string()
                } else {
                    unit.loaded_domains.join(" ")
                }
            );
            if plan_only {
                summary.planned += 1;
                let detail = format!("{why}; retirement not executed in this mode");
                outcomes.push(row(unit, "planned", false, detail));
                continue;
            }
            match service::retire_label(target, &unit.label, runner).await {
                Ok((state, detail)) if state == "retired" => {
                    summary.changed += 1;
                    outcomes.push(row(unit, "retired", true, format!("{why}: {detail}")));
                }
                Ok(_) => {}
                Err(error) => {
                    summary.failures += 1;
                    outcomes.push(row(unit, "repair_failed", false, error.to_string()));
                }
            }
        }
    }
    outcomes
}

fn scan_failed(outcomes: &mut Vec<ServiceReconcileOutcome>, host: &str, detail: &str) {
    outcomes.push(ServiceReconcileOutcome {
        host: host.to_string(),
        service: "undeclared-units".to_string(),
        unit: String::new(),
        beacon_state: "not-used".to_string(),
        endpoint_state: "not-used".to_string(),
        classification: "repair_failed".to_string(),
        action: "retire_stray".to_string(),
        changed: false,
        detail: detail.to_string(),
    });
}
