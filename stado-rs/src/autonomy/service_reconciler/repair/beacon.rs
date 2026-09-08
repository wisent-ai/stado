//! The one repair allowed to run on unknown evidence.

use crate::deploy::service::{self, ServiceStatus};

use super::plan::{replace_declaration, resolved_plan};

/// Repair the one unit whose death blinds every other repair.
///
/// A silent host beacon turns every service on that host `unknown`, and this
/// stage rightly refuses to mutate on unknown evidence — which would leave a
/// dead beacon dead forever, and with it the whole host unrepairable. The
/// beacon unit is the one exception: the evidence for "the beacon is down" is
/// the beacon's own absence, and the evidence that repair is possible is the
/// host channel answering. `ensure` leaves a matching retained definition
/// loaded; only an actual definition drift takes its preflighted, rollback-
/// guarded reload path.
///
/// The registry is only written for a registry-sourced declaration; a
/// recovery-sourced beacon stays owned by the fixed host-recovery program.
pub(in crate::autonomy::service_reconciler) async fn reconcile_beacon(
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
            "beacon ensure did not establish a running unit: {}",
            outcome.report.failure()
        ));
    }
    let mut declaration_changed = false;
    if status.service.source == service::SOURCE_REGISTRY {
        let corrected = service::record_from_ensure(
            &status.service.host,
            &status.service.name,
            &outcome,
            &status.service.managed_since,
        );
        declaration_changed =
            replace_declaration(&status.service, corrected, program, args, systemd_unit).await?;
    }
    Ok((
        outcome.action.clone(),
        outcome.changed() || declaration_changed,
        format!(
            "host beacon was silent; reasserted the beacon unit in the {} domain so evidence can resume",
            outcome.domain_word()
        ),
    ))
}
