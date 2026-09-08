//! Hand the stable bind back to the previous release, or to the legacy unit
//! when there is no previous release to hand it to.

use std::time::Duration;

use nix::sys::signal::{kill, Signal};
use nix::unistd::Pid;

use crate::release_agent::rollout::serving::answer::ensure_active_proxy;
use crate::release_agent::rollout::serving::discover::terminate;
use crate::release_agent::rollout::serving::legacy::restore_legacy;
use crate::release_agent::state::document::save_state;
use crate::release_agent::state::evidence::quarantine_with_logs;
use crate::release_agent::state::records::{HostReleaseState, RolloutPhase};
use crate::release_control::ReleaseTargetPolicy;

pub(crate) async fn rollback(
    target: &ReleaseTargetPolicy,
    state: &mut HostReleaseState,
    reason: String,
    readiness_timeout_seconds: u64,
) -> Result<(), String> {
    let failed = state.active.take().or_else(|| state.candidate.take());
    let reason = if let Some(record) = &failed {
        let failure = quarantine_with_logs(target, &state.product, record, &reason);
        let reason = failure.reason.clone();
        state
            .quarantined
            .insert(record.artifact_sha256.clone(), failure);
        reason
    } else {
        reason
    };
    if let Some(previous) = state.previous.take() {
        let serving = target.blue_green_serving()?;
        let product = state.product.clone();
        let generation = state.rollout_generation;
        ensure_active_proxy(
            target,
            &serving,
            &product,
            generation,
            &previous,
            state,
            readiness_timeout_seconds,
        )
        .await?;
        if let Some(record) = &failed {
            terminate(record);
        }
        state.active = Some(previous);
    } else {
        if let Some(proxy_pid) = state.proxy_pid.take() {
            let _ = kill(Pid::from_raw(proxy_pid), Signal::SIGTERM);
            tokio::time::sleep(Duration::from_millis(200)).await;
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
