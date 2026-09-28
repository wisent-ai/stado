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
//! The sweep only nominates. Every boot-out goes through the pass's mutation
//! gate (action limit, live pause and circuit breaker, the unit's lease), and
//! under that lease the registry is read again and the standby probed again:
//! a host promoted to the active one since the sweep, a withdrawn standby
//! address or a port now held by another job stops nothing.
//!
//! Only the boot-out is done. The unit file and its autostart stay, because a
//! move to this host starts the same unit, and a unit that comes back at login
//! is found by the next sweep and booted out again.

use crate::autonomy::policy::{AutonomyMode, AutonomyPolicy};
use crate::cli::service_verify::Finding;
use crate::deploy::service::{self, BootoutScope};
use crate::deploy::Runner;
use crate::queue::StorageError;

use super::super::gate::MutationGate;
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

/// What the re-check under the lease found and what was done.
enum Stop {
    /// The unit was booted out; whether anything was loaded, and the detail.
    Stopped(bool, String),
    /// The standby no longer serves, or is no longer a standby: nothing to do.
    Resolved(String),
}

/// Re-check the standby under its lease, then boot its declared unit out.
async fn stop_one(finding: &Finding, unit: &str, runner: &Runner) -> Result<Stop, String> {
    let service_name = &finding.service;
    if let Err(reason) =
        crate::cli::service_verify::standby_still_serving(service_name, &finding.host).await
    {
        return Ok(Stop::Resolved(format!(
            "re-checked under the lease: {reason}"
        )));
    }
    let target = crate::deploy::host_channel::canonical_target(&finding.host)
        .await
        .map_err(|error| error.to_string())?;
    let (state, detail) = service::bootout_label(&target, unit, BootoutScope::Any, runner)
        .await
        .map_err(|error| error.to_string())?;
    if state == "refused" || state == "failed" {
        return Err(format!("standby unit {unit} {state}: {detail}"));
    }
    let detail = format!("{}; standby unit {unit} {state}: {detail}", finding.detail);
    Ok(Stop::Stopped(state != "absent", detail))
}

/// Boot out the declared unit of every standby the sweep found serving.
/// Report mode and the emergency pause record the plan and touch nothing.
pub(in crate::autonomy::service_reconciler) async fn stop_serving_standbys(
    findings: &[Finding],
    policy: &AutonomyPolicy,
    runner: &Runner,
    gate: &mut MutationGate<'_>,
    summary: &mut ServiceReconcileSummary,
) -> Result<Vec<ServiceReconcileOutcome>, StorageError> {
    let serving: Vec<&Finding> = findings
        .iter()
        .filter(|finding| finding.state == crate::observations::STANDBY_SERVING)
        .collect();
    let mut outcomes = Vec::new();
    if serving.is_empty() {
        return Ok(outcomes);
    }
    let registry = match crate::targets::fetch_registry_or_last_good().await {
        Ok((registry, _)) => registry,
        Err(error) => {
            for finding in serving {
                summary.failures += 1;
                let detail = format!("registry unreadable, standby not stopped: {error}");
                outcomes.push(row(finding, "", "repair_failed", false, detail));
            }
            return Ok(outcomes);
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
        summary.planned += 1;
        if plan_only {
            let detail = format!("{}; boot-out not executed in this mode", finding.detail);
            outcomes.push(row(finding, unit, "planned", false, detail));
            continue;
        }
        let (subject, lease) = match gate.admit(&finding.host, unit).await? {
            Ok(admitted) => admitted,
            Err(refusal) => {
                summary.blocked += 1;
                outcomes.push(row(
                    finding,
                    unit,
                    refusal.classification,
                    false,
                    refusal.detail,
                ));
                continue;
            }
        };
        let result = stop_one(finding, unit, runner).await;
        match gate.release(&subject, &lease, result).await {
            Ok(Stop::Stopped(changed, detail)) => {
                if changed {
                    summary.changed += 1;
                }
                gate.record(None).await?;
                outcomes.push(row(finding, unit, "stopped", changed, detail));
            }
            // A re-check that finds nothing to stop ran no host command, so it
            // is neither a change nor a failure the circuit breaker counts.
            Ok(Stop::Resolved(detail)) => {
                outcomes.push(row(finding, unit, "resolved", false, detail));
            }
            Err(error) => {
                summary.failures += 1;
                gate.record(Some(&error)).await?;
                outcomes.push(row(finding, unit, "repair_failed", false, error));
            }
        }
    }
    Ok(outcomes)
}
