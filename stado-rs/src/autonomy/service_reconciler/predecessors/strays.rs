//! Units in the fleet's own label namespace that the registry never declared
//! and that are failing, retired on every local host.
//!
//! `service list --undeclared` names them, and that was all: a stray copy of
//! Weles bootstrapped by hand as
//! `com.wisent.compute.service.com.wisent.always-on.weles` ran a program from
//! a loose `~/weles` folder on lukasz-macbook, exited 1 on every start and was
//! started again at every login, and no pass ever acted on the row. A unit the
//! registry does not declare has no owner that will repair it, so a failing
//! one is retired the way a catalog predecessor is: booted out and its
//! autostart withdrawn, the unit file left in place.
//!
//! Only the narrow class is touched. The label must carry the fleet prefix
//! (classification `undeclared`), hold no process, and have last exited
//! non-zero. A program under `~/.stado/` is left alone even then, because
//! Stado installs those itself and one may be ahead of the registry document
//! for a pass.

use crate::autonomy::policy::{AutonomyMode, AutonomyPolicy};
use crate::deploy::service::{self, UndeclaredUnit};
use crate::deploy::Runner;

use super::super::receipts::{ServiceReconcileOutcome, ServiceReconcileSummary};

/// The path segment of the directory Stado installs its own programs into.
const STADO_HOME_SEGMENT: &str = "/.stado/";

/// Is `unit` a failing unit in the fleet namespace that nothing declares and
/// nothing is running?
fn is_failing_stray(unit: &UndeclaredUnit) -> bool {
    let exited_nonzero = unit
        .last_exit
        .or_else(|| unit.status.trim().parse::<i64>().ok())
        .is_some_and(|code| code != i64::default());
    unit.classification() == "undeclared"
        && unit.pid.trim().is_empty()
        && exited_nonzero
        && !unit.declared_program().contains(STADO_HOME_SEGMENT)
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

/// Retire every failing undeclared fleet unit on each local host. Report mode
/// and the emergency pause record the plan and touch nothing.
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
        for unit in units.iter().filter(|unit| is_failing_stray(unit)) {
            let program = unit.declared_program();
            let why = format!(
                "undeclared, not running, last exit {}, program {program}",
                unit.status
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
