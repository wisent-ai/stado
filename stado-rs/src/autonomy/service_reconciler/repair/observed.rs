//! The two endpoint-evidenced repairs: adopt a responding endpoint's unit,
//! and re-establish a unit whose endpoint was proven silent.

use crate::deploy::service::{self, ServiceStatus};

use super::plan::{replace_declaration, resolved_plan};

pub(in crate::autonomy::service_reconciler) async fn reconcile_observed(
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
            "endpoint responds, but the declared unit could not be inspected: {}",
            report.failure()
        ));
    }
    if report.unit_state != "loaded" {
        return Err(
            "endpoint responds, but the declared unit is not loaded; refusing to create a duplicate until its owning unit or process is identified"
                .to_string(),
        );
    }
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
    if running.matches_process() != Some(true) {
        return Err(format!(
            "endpoint responds and unit {} is loaded, but ownership is not proven by its running program",
            plan.label
        ));
    }
    let changed =
        replace_declaration(&status.service, corrected, program, args, systemd_unit).await?;
    let action = if changed { "adopted" } else { "confirmed" };
    Ok((
        action.to_string(),
        changed,
        format!(
            "responding endpoint has a loaded unit {} running its declared program",
            plan.label
        ),
    ))
}

pub(in crate::autonomy::service_reconciler) async fn reconcile_unreachable(
    status: &ServiceStatus,
    target: &crate::targets::ComputeTarget,
    runner: &crate::deploy::Runner,
) -> Result<(String, bool, String), String> {
    let (plan, program, args, systemd_unit) = resolved_plan(status, target)?;
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
            "unit and endpoint were absent; ensure completed in {} domain",
            outcome.domain_word()
        ),
    ))
}
