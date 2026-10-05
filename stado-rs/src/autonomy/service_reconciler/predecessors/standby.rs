//! A standby unit found serving, stopped on its own host.
//!
//! A standby host is by the registry's definition not running the service: it
//! holds an address it would serve on after a move. `service verify` dials
//! that address from the standby host and files `standby_serving` when the
//! unit the registry declares there holds the port, which is a second copy
//! keeping its own state beside the active host (for Skarbiec, a vault that
//! takes writes the owner never sees). Reporting it left the copy running, so
//! each pass now stops that one declared managed service.
//!
//! The sweep only nominates. Every stop goes through the pass's mutation gate
//! (action limit, live pause and circuit breaker, the unit's lease), and under
//! that lease the registry is read from its authority, uncached, and the
//! standby probed again: a host promoted to the active one since the sweep, a
//! withdrawn standby address or a port now held by another job stops nothing,
//! and an authority that does not answer stops nothing either.
//!
//! The stop is the managed-service stop a fenced cutover uses
//! (`service::stop_service`): the declaration's own unit file decides the
//! domain (a system LaunchDaemon, a per-user agent, a systemd unit), and the
//! unit stays enabled and registered, so a move to this host still starts it.
//! A copy that comes back at login is found by the next sweep and stopped
//! again. A system LaunchDaemon needs the host account's password, which this
//! pass does not hold, so such a stop is a `repair_failed` naming that.

use crate::autonomy::policy::{AutonomyMode, AutonomyPolicy};
use crate::cli::service_verify::{Finding, StandbyRecheck};
use crate::deploy::service::{self, ManagedService};
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
    /// The managed service was stopped; the host's report.
    Stopped(String),
    /// The standby no longer serves, or is no longer a standby: nothing to do.
    Resolved(String),
    /// The registry authority did not answer, so nothing was concluded.
    Unjudged(String),
}

/// Re-check the standby under its lease, then stop its declared service.
async fn stop_one(
    finding: &Finding,
    declared: &ManagedService,
    runner: &Runner,
) -> Result<Stop, String> {
    let unit = declared.unit_id();
    let recheck =
        crate::cli::service_verify::standby_still_serving(&finding.service, &finding.host, unit);
    match recheck.await {
        StandbyRecheck::Serving => {}
        StandbyRecheck::Settled(reason) => {
            return Ok(Stop::Resolved(format!(
                "re-checked under the lease: {reason}"
            )))
        }
        StandbyRecheck::Unjudged(reason) => {
            return Ok(Stop::Unjudged(format!(
                "not stopped, re-check under the lease was refused: {reason}"
            )))
        }
    }
    let target = crate::deploy::host_channel::canonical_target(&finding.host)
        .await
        .map_err(|error| error.to_string())?;
    let report = service::stop_service(&target, declared, runner)
        .await
        .map_err(|error| error.to_string())?;
    if !report.postcondition_held() {
        return Err(format!("standby service {unit}: {}", report.failure()));
    }
    Ok(Stop::Stopped(format!(
        "{}; standby service {unit} {} in {} ({})",
        finding.detail, report.status, report.unit, report.domain
    )))
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
    // The unit a destructive action names comes from the authority itself;
    // a last-known-good copy may name a host promoted since.
    let registry = match crate::targets::fetch_registry_remote().await {
        Ok(registry) => registry,
        Err(error) => {
            for finding in serving {
                summary.blocked += 1;
                let detail = format!("registry authority did not answer, not stopped: {error}");
                outcomes.push(row(finding, "", "stop_unjudged", false, detail));
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
        let declared = registry
            .local_targets()
            .into_iter()
            .find(|target| target.name == finding.host)
            .and_then(|target| {
                service::declared_services(target)
                    .into_iter()
                    .find(|found| found.unit_id() == unit)
            });
        let Some(declared) = declared else {
            summary.failures += 1;
            let detail = format!(
                "{}; {} declares no managed service {unit}, so nothing was stopped",
                finding.detail, finding.host
            );
            outcomes.push(row(finding, unit, "repair_failed", false, detail));
            continue;
        };
        summary.planned += 1;
        if plan_only {
            let detail = format!("{}; stop not executed in this mode", finding.detail);
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
        let result = stop_one(finding, &declared, runner).await;
        match gate.release(&subject, &lease, result).await {
            Ok(Stop::Stopped(detail)) => {
                summary.changed += 1;
                gate.record(None).await?;
                outcomes.push(row(finding, unit, "stopped", true, detail));
            }
            // A re-check that finds nothing to stop ran no host command, so it
            // is neither a change nor a failure the circuit breaker counts.
            Ok(Stop::Resolved(detail)) => {
                outcomes.push(row(finding, unit, "resolved", false, detail));
            }
            Ok(Stop::Unjudged(detail)) => {
                summary.blocked += 1;
                outcomes.push(row(finding, unit, "stop_unjudged", false, detail));
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
