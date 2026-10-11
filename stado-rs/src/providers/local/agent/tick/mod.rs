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

use crate::providers::local::helpers;
use crate::providers::local::self_terminate;
use crate::providers::local::slots::ActiveSlot;
use crate::queue::capacity::CapacitySnapshot;

use reconcile::{GpuPowerLimitState, PlacementPolicyState};

use super::{agent_log, claim, Step};

/// Whether `error` is the fleet store not answering: a 5xx from the object
/// API or a transport failure reaching it. Such a tick did not run; it is not
/// an error in the agent.
fn store_unavailable(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        if let Some(storage) = cause.downcast_ref::<crate::queue::StorageError>() {
            return match storage {
                crate::queue::StorageError::Stado { status, .. }
                | crate::queue::StorageError::Gcs { status, .. } => *status >= 500,
                crate::queue::StorageError::Held(_) => true,
                crate::queue::StorageError::Http(http) => {
                    http.is_connect() || http.is_timeout() || http.is_request()
                }
                _ => false,
            };
        }
        cause
            .downcast_ref::<reqwest::Error>()
            .is_some_and(|http| http.is_connect() || http.is_timeout() || http.is_request())
    })
}

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
        gpu_type = helpers::detect_gpu_type().await;
    }
    let mut total_vram_gb = helpers::detect_local_vram_gb().await.max(1);
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
    } = prepare::start(kind, &gpu_type, total_vram_gb, poll, log_fn).await?;
    reap::release_left_behind(&store, &hostname, log_fn).await;
    let mut slots: Vec<ActiveSlot> = Vec::new();
    let mut agent_diag: Map<String, Value> = Map::new();
    let mut last_cap: Option<CapacitySnapshot> = None;
    let mut gpu_power_limit_state: Option<GpuPowerLimitState> = None;
    let mut placement_policy_state: Option<PlacementPolicyState> = None;
    // The registry's ComputeTarget.pinned_only, refreshed per poll.
    let mut pinned_only = false;
    // The free bytes the disk-full rule keeps: a fifth of the fleet's volume,
    // re-read every tick in `policy::disk_policy`.
    let mut disk_low_bytes: Option<i64> = None;
    let mut pace = false;
    loop {
        if pace {
            tokio::time::sleep(poll).await;
        }
        pace = true;
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
        let (registry_target, current_free_bytes, pressure_active) = tick_step!(
            heartbeat,
            policy::disk_policy(
                &store,
                &consumer_id,
                kind,
                &hostname,
                &fleet_staging,
                total_vram_gb,
                &slots,
                &mut agent_diag,
                &mut disk_low_bytes,
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
                kind,
                &gpu_type,
                total_vram_gb,
                free_vram_gb,
                idle_shutdown,
                pressure_active,
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
                pressure_active,
                current_free_bytes,
                disk_low_bytes,
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

        if started > 0 {
            pace = false;
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
