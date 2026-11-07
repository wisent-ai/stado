//! One poll of the agent loop, phase by phase: [`prepare`] advances the
//! running slots, [`policy`] reads the disk declaration this tick admits
//! against, [`reconcile`] re-asserts the registry's host-level declarations,
//! and [`gates`] decides what the broadcast says and whether anything is
//! claimed at all.

#[macro_use]
mod phase;
pub mod gates;
pub mod policy;
pub mod prepare;
mod reap;
pub mod reconcile;

use std::time::Duration;

use serde_json::{Map, Value};

use crate::primitives::tally::Tally;
use crate::providers::local::helpers;
use crate::providers::local::self_terminate;
use crate::providers::local::slots::ActiveSlot;
use crate::queue::capacity::CapacitySnapshot;

use reconcile::{GpuPowerLimitState, PlacementPolicyState};

use super::{agent_log, claim, Step};

/// Main agent loop. Polls queue, runs jobs when Vast.ai is idle.
/// Python `run_agent`.
///
/// idle_shutdown=true: exit cleanly once both: (a) no jobs are active and
/// (b) no queued job fits this consumer's measured resources. The scheduler
/// observes the stopped capacity heartbeat and releases ephemeral machines
/// through their owning provider adapters.
///
/// kind: capacity-broadcast label distinguishing physical workstations
/// (kind="local") from ephemeral cloud-agent VMs (kind="gcp", ...).
/// `poll` is the operator's period between polls; a poll that started a job
/// is followed at once by the next. No global error handler wraps the loop
/// body: unexpected errors crash the agent visibly (returned as Err) so the
/// operator can diagnose.
pub async fn run_agent(
    gpu_type: &str,
    idle_shutdown: bool,
    kind: &str,
    poll: Duration,
) -> anyhow::Result<()> {
    let log_fn = &mut |msg: &str| agent_log(msg);
    if super::POLL.set(poll).is_err() {
        anyhow::bail!("an agent already runs in this process with its own poll period");
    }

    let mut gpu_type = gpu_type.to_string();
    if gpu_type.is_empty() {
        gpu_type = helpers::detect_gpu_type().await.map_err(|cause| {
            anyhow::anyhow!("the accelerator this host has cannot be read: {cause}")
        })?;
    }
    // A host without accelerator memory reports none; nothing here rounds it
    // up to a capacity it does not have.
    let mut total_vram_gb = helpers::detect_local_vram_gb()
        .await
        .map_err(|cause| anyhow::anyhow!("this host's VRAM cannot be read: {cause}"))?;
    let initial_gpu = gpu_type.clone();
    let prepare::Started {
        hostname,
        consumer_id,
        store,
        storage_backend,
        store_answers_for_fleet,
        sizing,
        fleet_staging,
        janitor_reports,
        janitor: _janitor,
        heartbeat,
        heartbeat_task: _heartbeat,
    } = prepare::start_when_store_answers(kind, &gpu_type, total_vram_gb, poll, log_fn).await?;
    reap::release_left_behind(&store, &hostname, log_fn).await;
    let mut slots: Vec<ActiveSlot> = Vec::new();
    let mut agent_diag: Map<String, Value> = Map::new();
    let mut last_cap: Option<CapacitySnapshot> = None;
    let mut gpu_power_limit_state: Option<GpuPowerLimitState> = None;
    let mut placement_policy_state: Option<PlacementPolicyState> = None;
    // The registry's ComputeTarget.pinned_only, refreshed per poll.
    let mut pinned_only = false;
    // One tick at once, then one every `poll`; a tick that started a job is
    // followed at once by the next.
    let mut schedule = tokio::time::interval(poll);
    schedule.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        schedule.tick().await;
        let vast_active = tick_phase!(
            heartbeat,
            prepare::advance_slots(
                &store,
                &sizing,
                &heartbeat,
                &janitor_reports,
                storage_backend,
                store_answers_for_fleet,
                &last_cap,
                &mut slots,
                &mut agent_diag,
                log_fn,
            )
            .await,
            log_fn
        );
        // The janitor's latest completed pass, read without waiting: its
        // margin is part of the room work here needs.
        let janitor_report = janitor_reports.margin();
        let (registry_target, room_short) = tick_step!(
            heartbeat,
            policy::disk_policy(
                &store,
                &consumer_id,
                kind,
                &hostname,
                &fleet_staging,
                total_vram_gb,
                &slots,
                &janitor_report,
                &mut agent_diag,
                &mut last_cap,
                log_fn,
            )
            .await,
            log_fn
        );
        tick_step!(
            heartbeat,
            reconcile::registry_declarations(
                &store,
                &consumer_id,
                kind,
                &initial_gpu,
                &registry_target,
                &slots,
                &mut total_vram_gb,
                &mut pinned_only,
                &mut agent_diag,
                &mut gpu_power_limit_state,
                &mut placement_policy_state,
                &mut last_cap,
                log_fn,
            )
            .await,
            log_fn
        );
        // The grant that lets this host claim work with secrets is kept
        // alive here, before the scan that would need it.
        gates::grant::renew_if_due(log_fn).await;
        let (mut free_vram_gb, mut cards) = tick_step!(
            heartbeat,
            gates::inference::before_admission(
                &store,
                &sizing,
                &consumer_id,
                kind,
                &gpu_type,
                total_vram_gb,
                pinned_only,
                vast_active,
                &slots,
                &mut agent_diag,
                &mut last_cap,
                log_fn,
            )
            .await,
            log_fn
        );
        let available_accelerators = tick_step!(
            heartbeat,
            gates::resources::measure(
                &store,
                &sizing,
                &consumer_id,
                kind,
                &gpu_type,
                total_vram_gb,
                &slots,
                &mut free_vram_gb,
                &mut cards,
                &mut agent_diag,
                &mut last_cap,
                log_fn,
            )
            .await,
            log_fn
        );
        tick_step!(
            heartbeat,
            gates::admission::publish_and_admit(
                &store,
                &sizing,
                &consumer_id,
                registry_target.as_ref().map(|target| target.name.as_str()),
                kind,
                &gpu_type,
                total_vram_gb,
                free_vram_gb,
                idle_shutdown,
                room_short.as_deref(),
                available_accelerators,
                &mut slots,
                &mut agent_diag,
                &mut last_cap,
                log_fn,
            )
            .await,
            log_fn
        );
        let queued = tick_step!(
            heartbeat,
            claim::queue::claimable(
                &store,
                &consumer_id,
                kind,
                &gpu_type,
                total_vram_gb,
                free_vram_gb,
                pinned_only,
                room_short.as_deref(),
                &slots,
                &mut agent_diag,
                log_fn,
            )
            .await,
            log_fn
        );
        let started = tick_phase!(
            heartbeat,
            claim::scan::claim_scan(
                &store,
                &sizing,
                &hostname,
                &consumer_id,
                kind,
                &gpu_type,
                total_vram_gb,
                pinned_only,
                &queued,
                &cards,
                &last_cap,
                &mut slots,
                &mut free_vram_gb,
                &mut agent_diag,
                log_fn,
            )
            .await,
            log_fn
        );

        if !started.counts_nothing() {
            schedule.reset_immediately();
            continue;
        }

        if idle_shutdown
            && slots.is_empty()
            && tick_phase!(
                heartbeat,
                helpers::no_eligible_in_queue(
                    &store,
                    &sizing,
                    &gpu_type,
                    total_vram_gb,
                    free_vram_gb,
                    kind,
                    &consumer_id,
                    slots.len(),
                )
                .await,
                log_fn
            )
        {
            log_fn("idle_shutdown: no slots + no eligible queued jobs; exiting");
            self_terminate(kind, log_fn).await;
            return Ok(());
        }
    }
}
