//! Whether a declared unit's work already runs inside a product's one
//! process on the same host, asked before the pass repairs that unit.
//!
//! Retiring a predecessor does not remove its registry declaration, so on the
//! next pass it reads `missing` and would be reasserted beside the process
//! that took its work over. The pass asks the same questions retirement asks,
//! from what the declaration runs ([`service::declared_owner`],
//! [`service::declared_role`]) and [`service::role_retired`], so a unit is
//! never both retired and repaired on one host, and a unit handed over to a
//! resolver that has not answered yet is not brought back before that
//! resolver has tried.

use crate::deploy::service::{self, ManagedService};
use crate::deploy::Runner;

use super::Replacement;

/// The sentence for a declaration that runs a catalog product's program
/// under a label that is not that product's unit, for every product but the
/// host Stado process, whose units are judged role by role in
/// [`taken_over`]. `None` otherwise, including when the host cannot be read.
pub(in crate::autonomy::service_reconciler) async fn replaced(
    declared: &ManagedService,
) -> Option<String> {
    let target = crate::deploy::host_channel::canonical_target(&declared.host)
        .await
        .ok()?;
    let owner = service::declared_owner(&target, declared).ok()??;
    let host = crate::deploy::service_catalog::host_process().ok()?;
    (owner.name != host.name)
        .then(|| crate::deploy::service_catalog::retired_sentence(declared.unit_id(), &owner))
}

/// `Some(detail)` when `declared` does the work of a role of a replacement
/// on its host and that replacement took the role over: its live process is
/// proven to run an ordinary role; for a role that shares its unit's
/// listener, the host's handoff record says the listener was acquired or the
/// unit stepped aside and the resolver has not answered; for the API
/// listener, the host Stado process recorded that it retired the unit at API
/// start. The two recorded kinds are asked even while the replacement is not
/// running, because a taken-over listener stays taken over across the
/// replacement's restarts. `None` when the unit is still the only thing doing
/// its work, including when that cannot be established.
pub(in crate::autonomy::service_reconciler) async fn taken_over(
    declared: &ManagedService,
    replacements: &[Replacement],
    runner: &Runner,
) -> Option<String> {
    let unit = declared.unit_id();
    for super::Replacement {
        service: running,
        entry,
        active,
    } in replacements
    {
        if running.host != declared.host {
            continue;
        }
        let target = crate::deploy::host_channel::canonical_target(&running.host)
            .await
            .ok()?;
        let Some(role) =
            service::declared_role(&target, entry, active.then_some(running), declared, runner)
                .await
        else {
            continue;
        };
        if !active
            && !service::listener_role(&role)
            && !crate::deploy::service_catalog::api_role(&role)
        {
            continue;
        }
        if let Some(proof) = service::role_retired(&target, running, &role, !active, runner).await {
            return Some(format!(
                "{unit} is retired on {}: {} took its role over ({proof})",
                running.host, entry.name
            ));
        }
    }
    None
}

/// After a repair of `declared` on `target`, whether it succeeded or not:
/// when it does the host Stado process's API listener work and that process
/// recorded its takeover meanwhile, retire it again, because that takeover
/// may have retired it before this repair's ensure brought it back. The
/// takeover records itself before it retires, so whichever of the two
/// finishes last, the unit ends retired. `Ok` with the retirement's detail,
/// `Err` when that retirement failed and the unit may run beside its
/// replacement; `None` when there was nothing to undo.
pub(in crate::autonomy::service_reconciler) async fn retake(
    declared: &ManagedService,
    target: &crate::targets::ComputeTarget,
    runner: &Runner,
) -> Option<Result<String, String>> {
    let host = crate::deploy::service_catalog::host_process().ok()?;
    let role = service::declared_role(target, &host, None, declared, runner).await?;
    if !crate::deploy::service_catalog::api_role(&role) {
        return None;
    }
    let unit = declared.unit_id();
    let retired = service::retire_if_taken_over(target, unit, runner).await?;
    let detail = format!(
        "{unit} was taken over by {} during this repair; retiring it again: {} ({})",
        crate::deploy::service_catalog::unit_of(&host),
        retired.state,
        retired.detail
    );
    Some(if retired.state == "failed" {
        Err(detail)
    } else {
        Ok(detail)
    })
}
