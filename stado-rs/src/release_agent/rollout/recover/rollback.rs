//! Hand the stable bind back to the previous release, or to the legacy unit
//! when there is no previous release to hand it to.

use crate::release_agent::rollout::serving::answer::ensure_active_proxy;
use crate::release_agent::rollout::serving::discover::terminate;
use crate::release_agent::rollout::serving::legacy::restore_legacy;
use crate::release_agent::state::document::save_state;
use crate::release_agent::state::evidence::quarantine_with_logs;
use crate::release_agent::state::records::{HostReleaseState, RolloutPhase};
use crate::release_cause::Refusal;
use crate::release_control::{ReleaseTargetPolicy, RolloutStrategy};

pub(crate) async fn rollback(
    target: &ReleaseTargetPolicy,
    state: &mut HostReleaseState,
    reason: Refusal,
    strategy: &RolloutStrategy,
) -> Result<(), String> {
    let failed = state.active.take().or_else(|| state.candidate.take());
    // The failed release's readiness loss is not the previous release's.
    state.readiness_lost_at = None;
    let reason = if let Some(record) = &failed {
        let failure = quarantine_with_logs(target, &state.product, record, &reason);
        let reason = failure.reason.clone();
        state
            .quarantined
            .insert(record.artifact_sha256.clone(), failure);
        reason
    } else {
        reason.sentence
    };
    if let Some(previous) = state.previous.take() {
        let serving = target.blue_green_serving()?;
        let product = state.product.clone();
        let generation = state.rollout_generation;
        // A previous release that does not answer cannot take the bind back;
        // that is said, not waited for.
        if let Some(why) = ensure_active_proxy(
            target, &serving, &product, generation, &previous, state, strategy,
        )
        .await?
        {
            return Err(format!(
                "{product} rollback cannot hand the stable bind back to its previous release \
                 on port {}: it does not answer: {why}",
                previous.port
            ));
        }
        if let Some(record) = &failed {
            terminate(record);
        }
        state.active = Some(previous);
    } else {
        if state.proxy_pid.take().is_some() {
            let serving = target.blue_green_serving()?;
            crate::release_agent::rollout::serving::control::stop(
                Some(&target.home),
                &crate::release_agent::state::document::proxy_state_path(target, &state.product),
                &serving.stable_bind,
            )
            .await?;
        }
        restore_legacy(target)?;
        if let Some(record) = &failed {
            terminate(record);
        }
    }
    state.candidate = None;
    state.phase = RolloutPhase::RolledBack;
    state.detail = reason;
    state.cutover_at = None;
    save_state(target, state)
}
