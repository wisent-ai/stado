//! A standby unit found serving, booted out on its own host.
//!
//! A standby host is by the registry's definition not running the service: it
//! holds an address it would serve on after a move. `service verify` dials
//! that address from the standby host and files `standby_serving` when the
//! unit the registry declares there holds the port, which is a second copy
//! keeping its own state beside the active host (for Skarbiec, a vault that
//! takes writes the owner never sees). Reporting it left the copy running, so
//! each pass now boots that one declared unit out of its host's init system.
//!
//! Only the boot-out is done. The unit file and its autostart stay, because a
//! move to this host starts the same unit, and a unit that comes back at login
//! is found by the next sweep and booted out again.

use crate::autonomy::policy::{AutonomyMode, AutonomyPolicy};
use crate::cli::service_verify::Finding;
use crate::deploy::service::{self, BootoutScope};
use crate::deploy::Runner;

use super::super::receipts::{ServiceReconcileOutcome, ServiceReconcileSummary};

fn row(
    finding: &Finding,
    unit: &str,
    classification: &str,
    changed: bool,
    detail: String,
) -> ServiceReconcileOutcome {
    ServiceReconcileOutcome {
        host: finding.host.clone(),
        service: finding.service.clone(),
        unit: unit.to_string(),
        beacon_state: "not-used".to_string(),
        endpoint_state: finding.state.to_string(),
        classification: classification.to_string(),
        action: "stop_standby".to_string(),
        changed,
        detail,
    }
}

/// Boot out the declared unit of every standby the sweep found serving.
/// Report mode and the emergency pause record the plan and touch nothing.
pub(in crate::autonomy::service_reconciler) async fn stop_serving_standbys(
    findings: &[Finding],
    policy: &AutonomyPolicy,
    runner: &Runner,
    summary: &mut ServiceReconcileSummary,
) -> Vec<ServiceReconcileOutcome> {
    let serving: Vec<&Finding> = findings
        .iter()
        .filter(|finding| finding.state == crate::observations::STANDBY_SERVING)
        .collect();
    let mut outcomes = Vec::new();
    if serving.is_empty() {
        return outcomes;
    }
    let registry = match crate::targets::fetch_registry_or_last_good().await {
        Ok((registry, _)) => registry,
        Err(error) => {
            for finding in serving {
                summary.failures += 1;
                let detail = format!("registry unreadable, standby not stopped: {error}");
                outcomes.push(row(finding, "", "repair_failed", false, detail));
            }
            return outcomes;
        }
    };
    let plan_only = policy.mode == AutonomyMode::Report || policy.emergency_paused;
    for finding in serving {
        let Some(unit) = registry.service_unit(&finding.service, &finding.host) else {
            summary.failures += 1;
            let detail = format!(
                "{}; the registry names no unit for {} on {}, so nothing was stopped",
                finding.detail, finding.service, finding.host
            );
            outcomes.push(row(finding, "", "repair_failed", false, detail));
            continue;
        };
        if plan_only {
            summary.planned += 1;
            let detail = format!("{}; boot-out not executed in this mode", finding.detail);
            outcomes.push(row(finding, unit, "planned", false, detail));
            continue;
        }
        let target = match crate::deploy::host_channel::canonical_target(&finding.host).await {
            Ok(target) => target,
            Err(error) => {
                summary.failures += 1;
                outcomes.push(row(finding, unit, "repair_failed", false, error.to_string()));
                continue;
            }
        };
        match service::bootout_label(&target, unit, BootoutScope::Any, runner).await {
            Ok((state, detail)) if state != "refused" && state != "failed" => {
                let changed = state != "absent";
                if changed {
                    summary.changed += 1;
                }
                let detail = format!("{}; standby unit {unit} {state}: {detail}", finding.detail);
                outcomes.push(row(finding, unit, "stopped", changed, detail));
            }
            Ok((state, detail)) => {
                summary.failures += 1;
                let detail = format!("standby unit {unit} {state}: {detail}");
                outcomes.push(row(finding, unit, "repair_failed", false, detail));
            }
            Err(error) => {
                summary.failures += 1;
                outcomes.push(row(finding, unit, "repair_failed", false, error.to_string()));
            }
        }
    }
    outcomes
}
