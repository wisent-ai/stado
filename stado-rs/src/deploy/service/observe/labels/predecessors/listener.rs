//! One step of a listener handoff for a role unit, as the reconciler takes
//! it: judge the handoff, and act on the judgement. The reconciler holds the
//! unit's lease across [`hand_over_role`], so the record it reads is the one
//! it writes; [`listener_standing`] only reads, to decide whether a lease is
//! needed at all.

use crate::deploy::service::*;

use super::{retirement, role_process, PredecessorRetirement};

/// Whether `role` shares its old unit's listener, so its unit is handed over
/// rather than retired on the flag.
pub fn listener_role(role: &crate::deploy::service_catalog::RoleUnit) -> bool {
    role.readiness.as_deref() == Some(RESOLVER_STATE)
}

/// The replacement's process and why it does not run the role. A replacement
/// that cannot even be inspected runs no role, and the handoff under way for
/// it is still judged, so the unit that stepped aside is never lost.
async fn replacement(
    target: &ComputeTarget,
    running: &ManagedService,
    role: &crate::deploy::service_catalog::RoleUnit,
    runner: &Runner,
) -> (RunningProgram, Option<String>) {
    match role_process(target, running, &role.flag, runner).await {
        Ok(found) => found,
        Err(error) => (
            RunningProgram::default(),
            Some(format!(
                "{} could not be inspected: {error}",
                running.unit_id()
            )),
        ),
    }
}

/// Where `role`'s handoff stands on `target`, without changing anything.
pub async fn listener_standing(
    target: &ComputeTarget,
    running: &ManagedService,
    role: &crate::deploy::service_catalog::RoleUnit,
    stopped: bool,
    runner: &Runner,
) -> Result<Handoff, DeployError> {
    let (process, not_running) = replacement(target, running, role, runner).await;
    handoff_standing(
        target,
        &process,
        not_running.as_deref(),
        stopped,
        &role.unit,
        runner,
    )
    .await
}

/// Judge `role`'s handoff again and act on it: record an acquired listener,
/// start a handoff when `may_start`, or undo one that did not take. `stopped`
/// says the registry holds the replacement stopped.
pub async fn hand_over_role(
    target: &ComputeTarget,
    running: &ManagedService,
    role: &crate::deploy::service_catalog::RoleUnit,
    may_start: bool,
    stopped: bool,
    runner: &Runner,
) -> PredecessorRetirement {
    let unit = role.unit.clone();
    let answer = |state: &str, detail: String| PredecessorRetirement {
        unit: unit.clone(),
        state: state.to_string(),
        detail,
    };
    let (process, not_running) = replacement(target, running, role, runner).await;
    let standing = handoff_standing(
        target,
        &process,
        not_running.as_deref(),
        stopped,
        &unit,
        runner,
    )
    .await;
    let outcome = match standing {
        // The listener is recorded acquired before the old unit is retired and
        // complete only once that retirement is confirmed, so a retirement that
        // fails half way is retried on the next pass rather than forgotten.
        Ok(Handoff::Complete) => {
            match mark_handoff(target, &unit, "acquired", &process, runner).await {
                Ok(()) => {
                    let retired = retirement(target, &unit, runner).await;
                    if retired.state == "failed" {
                        return retired;
                    }
                    match mark_handoff(target, &unit, "complete", &process, runner).await {
                        Ok(()) => return retired,
                        Err(error) => Err(error),
                    }
                }
                Err(error) => Err(error),
            }
        }
        Ok(Handoff::Retained(detail)) => return answer("absent", detail),
        Ok(Handoff::Kept(detail)) => return answer("kept", detail),
        Ok(Handoff::Waiting(detail)) => Ok(("awaiting_resolver".to_string(), detail)),
        Ok(Handoff::Start) if !may_start => {
            return answer(
                "kept",
                format!(
                    "{} runs the resolver role, but only the reconciler of a host that declares \
                     {unit} can hand it over and bring it back",
                    running.unit_id()
                ),
            )
        }
        Ok(Handoff::Start) => start_handoff(target, &unit, &process, runner).await,
        Ok(Handoff::Restore {
            scopes,
            artefact,
            detail,
        }) => restore_handoff(target, &unit, &artefact, &scopes, runner)
            .await
            .map(|()| ("restored".to_string(), detail)),
        Err(error) => Err(error),
    };
    match outcome {
        Ok((state, detail)) => answer(&state, detail),
        Err(error) => answer("failed", error.to_string()),
    }
}

/// Whether `role`'s unit must not be repaired: its listener was acquired, or
/// it stepped aside and the resolver has not answered yet. `stopped` says the
/// registry holds the replacement stopped, which undoes an unfinished
/// handoff but never a completed one.
pub(super) async fn listener_retired(
    target: &ComputeTarget,
    running: &ManagedService,
    role: &crate::deploy::service_catalog::RoleUnit,
    stopped: bool,
    runner: &Runner,
) -> Option<String> {
    match listener_standing(target, running, role, stopped, runner)
        .await
        .ok()?
    {
        Handoff::Complete => Some(format!("the resolver in {} serves", running.unit_id())),
        Handoff::Retained(detail) | Handoff::Waiting(detail) => Some(detail),
        Handoff::Start | Handoff::Restore { .. } | Handoff::Kept(_) => None,
    }
}
