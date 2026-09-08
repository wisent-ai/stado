//! One product's own pass: repair the bind, read the declaration, and answer
//! with the state document whichever branch this tick belongs to.

use std::time::Duration;

use chrono::Utc;

use super::promote::promote_candidate;
use crate::release_agent::rollout::candidate::spawn::not_ready_because;
use crate::release_agent::rollout::processes::reconcile::reconcile_stable_proxy;
use crate::release_agent::rollout::processes::sweep::sweep_leaked_processes;
use crate::release_agent::rollout::recover::rollback::rollback;
use crate::release_agent::rollout::recover::wall::cause_hold;
use crate::release_agent::rollout::serving::answer::ensure_active_proxy;
use crate::release_agent::rollout::serving::discover::terminate;
use crate::release_agent::state::document::{load_state, save_state};
use crate::release_agent::state::records::{HostReleaseState, QuarantineRecord, RolloutPhase};
use crate::release_control::{self, ProductReleasePolicy, ReleaseControl, ReleaseTargetPolicy};

pub(crate) async fn reconcile_product(
    control: &ReleaseControl,
    product: &str,
    policy: &ProductReleasePolicy,
    target_name: &str,
    target: &ReleaseTargetPolicy,
) -> Result<HostReleaseState, String> {
    let mut state = load_state(target, product, target_name)?;
    let install_root = release_control::install_root_path(policy, target);
    let install_root = install_root
        .to_str()
        .ok_or_else(|| format!("{product} install root is not valid UTF-8"))?;
    // Repair the stable bind before any desired/quarantine branch can return.
    // `reconcile_once` holds the per-product lock across this state load,
    // declaration/world reconciliation, and every persisted repair below.
    reconcile_stable_proxy(
        target,
        product,
        install_root,
        policy.strategy.readiness_timeout_seconds,
        &mut state,
    )
    .await?;
    // `reconcile_once` hands only blue-green policies to this function; ask
    // for the serving coordinates by name rather than re-checking the
    // validator's invariant, so a replace policy reaching here fails loudly
    // instead of halfway through a rollout.
    let serving = target.blue_green_serving()?;
    // Reconcile the process world before reasoning from the record: anything
    // running out of this product's releases directory that the record does not
    // name is a leak from a run that died between spawning and saving.
    sweep_leaked_processes(target, product, install_root, &state);
    let Some(desired) = policy.desired.as_ref() else {
        state.phase = RolloutPhase::Idle;
        state.detail = "no desired release".to_string();
        save_state(target, &mut state)?;
        return Ok(state);
    };
    let artifact = desired
        .artifacts
        .get(&target.platform)
        .ok_or_else(|| format!("desired release has no {} artifact", target.platform))?;
    let repeats_failed_rollout = state.phase == RolloutPhase::RolledBack
        && state.rollout_generation == desired.rollout_generation
        && state.active.is_none()
        && state.previous.is_none();
    state.rollout_generation = desired.rollout_generation;
    if repeats_failed_rollout {
        state
            .quarantined
            .entry(artifact.artifact_sha256.clone())
            .or_insert_with(|| QuarantineRecord::new(state.detail.clone()));
        state.phase = RolloutPhase::Quarantined;
        state.detail =
            "desired release digest is quarantined after its previous rollback".to_string();
        save_state(target, &mut state)?;
        return Ok(state);
    }

    if state.quarantined.contains_key(&artifact.artifact_sha256) {
        if let Some(active) = state.active.clone() {
            if let Err(reason) = ensure_active_proxy(
                target,
                &serving,
                product,
                desired.rollout_generation,
                &active,
                &mut state,
                policy.strategy.readiness_timeout_seconds,
            )
            .await
            {
                if policy.strategy.automatic_rollback {
                    rollback(
                        target,
                        &mut state,
                        reason,
                        policy.strategy.readiness_timeout_seconds,
                    )
                    .await?;
                } else {
                    state.phase = RolloutPhase::Failed;
                    state.detail = reason;
                    save_state(target, &mut state)?;
                }
                return Ok(state);
            }
        }
        state.phase = RolloutPhase::Quarantined;
        state.detail = "desired release digest is quarantined on this host".to_string();
        save_state(target, &mut state)?;
        return Ok(state);
    }

    if state
        .active
        .as_ref()
        .is_some_and(|active| active.artifact_sha256 == artifact.artifact_sha256)
    {
        let active = state.active.clone().expect("checked above");
        let proxy_result = ensure_active_proxy(
            target,
            &serving,
            product,
            desired.rollout_generation,
            &active,
            &mut state,
            policy.strategy.readiness_timeout_seconds,
        )
        .await;
        if let Err(reason) = proxy_result {
            if policy.strategy.automatic_rollback {
                rollback(
                    target,
                    &mut state,
                    reason,
                    policy.strategy.readiness_timeout_seconds,
                )
                .await?;
            } else {
                state.phase = RolloutPhase::Failed;
                state.detail = reason;
                save_state(target, &mut state)?;
            }
            return Ok(state);
        }
        if !matches!(
            state.phase,
            RolloutPhase::Monitoring | RolloutPhase::Committed
        ) {
            state.phase = RolloutPhase::Routed;
            state.detail = format!("stable proxy routed to candidate port {}", active.port);
            state.cutover_at.get_or_insert_with(Utc::now);
            save_state(target, &mut state)?;
            tokio::time::sleep(Duration::from_secs(policy.strategy.drain_timeout_seconds)).await;
            if let Some(why) = not_ready_because(&active, &serving.readiness_path).await {
                if policy.strategy.automatic_rollback {
                    rollback(
                        target,
                        &mut state,
                        format!("candidate failed during drain: {why}"),
                        policy.strategy.readiness_timeout_seconds,
                    )
                    .await?;
                } else {
                    state.phase = RolloutPhase::Failed;
                    state.detail = format!(
                        "candidate failed during drain: {why}; automatic rollback is disabled"
                    );
                    save_state(target, &mut state)?;
                }
                return Ok(state);
            }
            state.phase = RolloutPhase::Monitoring;
            state.detail = "previous release drained and retained for rollback window".to_string();
            save_state(target, &mut state)?;
        }
        if state.phase == RolloutPhase::Monitoring {
            let elapsed = state
                .cutover_at
                .map(|cutover| {
                    Utc::now()
                        .signed_duration_since(cutover)
                        .num_seconds()
                        .max(0) as u64
                })
                .unwrap_or_default();
            if elapsed >= policy.strategy.rollback_window_seconds {
                if let Some(previous) = state.previous.take() {
                    terminate(&previous);
                }
                state.phase = RolloutPhase::Committed;
                state.detail = "release committed after rollback window".to_string();
                save_state(target, &mut state)?;
            }
        }
        return Ok(state);
    }

    // Everything below stages and burns a new candidate. Before spending one,
    // ask the cause's own condition whether the wall is still there, and fall
    // back to counting only when there is nothing to ask or it cannot answer.
    // This sits AFTER the desired-digest quarantine guard above, which returns
    // first and is untouched.
    if let Some(hold) = cause_hold(target, &state).await {
        state.phase = RolloutPhase::Quarantined;
        state.detail = hold.sentence();
        save_state(target, &mut state)?;
        return Ok(state);
    }

    promote_candidate(
        control, product, policy, target, &serving, desired, artifact, &mut state,
    )
    .await?;
    Ok(state)
}
