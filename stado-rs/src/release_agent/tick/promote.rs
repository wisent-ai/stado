//! Stage and burn one new candidate: fetch it, verify it, install it, start
//! it, route the stable bind to it, and watch it drain.

use std::time::Duration;

use chrono::Utc;

use crate::release_agent::rollout::candidate::fetch::fetch_candidate;
use crate::release_agent::rollout::candidate::spawn::{
    await_ready_because, not_ready_because, spawn_release,
};
use crate::release_agent::rollout::candidate::stage::{next_port, stage_release};
use crate::release_agent::rollout::recover::rollback::rollback;
use crate::release_agent::rollout::serving::answer::ensure_active_proxy;
use crate::release_agent::rollout::serving::discover::terminate;
use crate::release_agent::rollout::serving::proxy::proxy_upstream_port;
use crate::release_agent::state::document::save_state;
use crate::release_agent::state::evidence::quarantine_with_logs;
use crate::release_agent::state::records::{HostReleaseState, QuarantineRecord, RolloutPhase};
use crate::release_control::{
    BlueGreenServing, DesiredRelease, ProductReleasePolicy, ReleaseArtifactRef, ReleaseControl,
    ReleaseTargetPolicy,
};

/// Everything [`reconcile_product`] does once it has decided to spend a
/// candidate on this digest.
///
/// Every early return is the outcome it was inside that function: the state
/// document is already committed, and the caller answers with it.
///
/// [`reconcile_product`]: super::product::reconcile_product
// The coordinates a promotion needs are exactly the ones its caller already
// established; re-deriving any of them here would be a second answer to what
// this rollout is.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn promote_candidate(
    control: &ReleaseControl,
    product: &str,
    policy: &ProductReleasePolicy,
    target: &ReleaseTargetPolicy,
    serving: &BlueGreenServing,
    desired: &DesiredRelease,
    artifact: &ReleaseArtifactRef,
    state: &mut HostReleaseState,
) -> Result<(), String> {
    if let Some(incomplete) = state.candidate.take() {
        terminate(&incomplete);
        state.detail = "discarded incomplete candidate from an interrupted rollout".to_string();
        save_state(target, state)?;
    }

    state.phase = RolloutPhase::Downloaded;
    state.detail = format!("fetching {} {}", product, desired.version);
    save_state(target, state)?;
    let (manifest, archive, directory) =
        match fetch_candidate(control, product, desired, artifact, policy, target).await {
            Ok(candidate) => candidate,
            Err(reason) => {
                state.quarantined.insert(
                    artifact.artifact_sha256.clone(),
                    QuarantineRecord::new(reason.clone()),
                );
                state.phase = RolloutPhase::Quarantined;
                state.detail = reason;
                save_state(target, state)?;
                return Ok(());
            }
        };
    state.phase = RolloutPhase::Verified;
    state.detail = "signature, provenance, qualification, schema, and digest verified".to_string();
    save_state(target, state)?;

    if let Some(active) = &state.active {
        if !manifest
            .rollback_compatible_with
            .iter()
            .any(|version| version == &active.version)
        {
            let reason = format!(
                "release {} does not declare rollback compatibility with {}",
                manifest.version, active.version
            );
            state.quarantined.insert(
                artifact.artifact_sha256.clone(),
                QuarantineRecord::new(reason.clone()),
            );
            state.phase = RolloutPhase::Quarantined;
            state.detail = reason;
            save_state(target, state)?;
            return Ok(());
        }
    }

    stage_release(&manifest, &archive, &directory)?;
    state.phase = RolloutPhase::Staged;
    state.detail = format!("staged immutable release at {}", directory.display());
    save_state(target, state)?;

    let port = next_port(
        serving.candidate_ports,
        state,
        proxy_upstream_port(target, product),
    );
    let process = spawn_release(product, policy, target, &manifest, &directory, port)?;
    state.candidate = Some(process.clone());
    state.phase = RolloutPhase::CandidateRunning;
    state.detail = format!("candidate pid={} port={port}", process.pid);
    save_state(target, state)?;
    if let Some(why) = await_ready_because(
        &process,
        &serving.readiness_path,
        policy.strategy.readiness_timeout_seconds,
    )
    .await
    {
        terminate(&process);
        let failure = quarantine_with_logs(
            target,
            product,
            &process,
            &format!(
                "candidate did not become ready within {}s: {why}",
                policy.strategy.readiness_timeout_seconds
            ),
        );
        let reason = failure.reason.clone();
        state
            .quarantined
            .insert(process.artifact_sha256.clone(), failure);
        state.candidate = None;
        state.phase = RolloutPhase::Quarantined;
        state.detail = reason;
        save_state(target, state)?;
        return Ok(());
    }

    state.previous = state.active.take();
    state.active = state.candidate.take();
    state.phase = RolloutPhase::Ready;
    state.detail = "candidate readiness passed; stable cutover pending".to_string();
    state.cutover_at = Some(Utc::now());
    save_state(target, state)?;

    let proxy_result = ensure_active_proxy(
        target,
        serving,
        product,
        desired.rollout_generation,
        &process,
        state,
        policy.strategy.readiness_timeout_seconds,
    )
    .await;
    if let Err(reason) = proxy_result {
        if policy.strategy.automatic_rollback {
            rollback(
                target,
                state,
                reason,
                policy.strategy.readiness_timeout_seconds,
            )
            .await?;
        } else {
            state.phase = RolloutPhase::Failed;
            state.detail = reason;
            save_state(target, state)?;
        }
        return Ok(());
    }

    state.phase = RolloutPhase::Routed;
    state.detail = format!("stable proxy routed to candidate port {port}");
    save_state(target, state)?;
    tokio::time::sleep(Duration::from_secs(policy.strategy.drain_timeout_seconds)).await;
    let active = state
        .active
        .clone()
        .ok_or_else(|| "routed release lost its active process record".to_string())?;
    if let Some(why) = not_ready_because(&active, &serving.readiness_path).await {
        if policy.strategy.automatic_rollback {
            rollback(
                target,
                state,
                format!("candidate failed during drain: {why}"),
                policy.strategy.readiness_timeout_seconds,
            )
            .await?;
        } else {
            state.phase = RolloutPhase::Failed;
            state.detail =
                format!("candidate failed during drain: {why}; automatic rollback is disabled");
            save_state(target, state)?;
        }
        return Ok(());
    }
    state.phase = RolloutPhase::Monitoring;
    state.detail = "previous release drained and retained for rollback window".to_string();
    save_state(target, state)?;
    Ok(())
}
