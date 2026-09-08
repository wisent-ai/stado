//! The repair for a unit the service directory never mentioned.

use crate::deploy::service::{self, ServiceStatus};

use super::plan::{replace_declaration, resolved_plan};

/// A declared unit the service directory says nothing about has no endpoint
/// to disprove, so "endpoint absence was not proven" would block its repair
/// forever. The host channel is the evidence instead: the unit is probed on
/// the box, a loaded unit must prove its live program before the declaration
/// is corrected, and only a unit the host itself reports absent is ensured.
pub(in crate::autonomy::service_reconciler) async fn reconcile_undeclared(
    status: &ServiceStatus,
    target: &crate::targets::ComputeTarget,
    runner: &crate::deploy::Runner,
) -> Result<(String, bool, String), String> {
    let (plan, program, args, systemd_unit) = resolved_plan(status, target)?;
    let report = service::probe_service(target, status.service.unit_id(), runner)
        .await
        .map_err(|error| error.to_string())?;
    if !report.succeeded("probed") {
        return Err(format!(
            "the unit could not be inspected over the host channel: {}",
            report.failure()
        ));
    }
    if report.unit_state == "loaded" {
        let mut corrected = service::record_from_report(
            &status.service.host,
            status.service.host_heuristic.as_deref(),
            &status.service.name,
            &report,
            &status.service.managed_since,
        );
        corrected.program = program.clone();
        corrected.args = args.clone();
        let running = service::inspect_process(target, &corrected, runner)
            .await
            .map_err(|error| error.to_string())?;
        match running.matches_process() {
            Some(true) => {
                let changed =
                    replace_declaration(&status.service, corrected, program, args, systemd_unit)
                        .await?;
                let action = if changed { "adopted" } else { "confirmed" };
                return Ok((
                    action.to_string(),
                    changed,
                    format!(
                        "beacon omitted unit {}, but the host reports it loaded and running its declared program",
                        plan.label
                    ),
                ));
            }
            // `Some(false)` covers two different worlds and only one is a
            // conflict. A process executing a binary the unit never declared
            // is unknown ownership and stays refused. A process executing the
            // declared binary that was REWRITTEN after the process started is
            // the four-day stale-agent incident, and the in-place kick below
            // is precisely its repair.
            Some(false) => {
                let same_binary = running
                    .running_binary()
                    .is_some_and(|binary| binary == running.declared || binary == running.resolved);
                if !same_binary {
                    return Err(format!(
                        "unit {} is loaded on the host but ownership is not proven by its running program",
                        plan.label
                    ));
                }
            }
            // the in-place `ensure` kick below is the repair, not a risk.
            None => {}
        }
    }
    let outcome = service::ensure_service(target, &plan, runner)
        .await
        .map_err(|error| error.to_string())?;
    if !outcome.succeeded() {
        return Err(format!(
            "ensure did not establish a running unit: {}",
            outcome.report.failure()
        ));
    }
    let corrected = service::record_from_ensure(
        &status.service.host,
        &status.service.name,
        &outcome,
        &status.service.managed_since,
    );
    let declaration_changed =
        replace_declaration(&status.service, corrected, program, args, systemd_unit).await?;
    Ok((
        outcome.action.clone(),
        outcome.changed() || declaration_changed,
        format!(
            "the host itself reported the unit absent; ensure completed in {} domain",
            outcome.domain_word()
        ),
    ))
}
