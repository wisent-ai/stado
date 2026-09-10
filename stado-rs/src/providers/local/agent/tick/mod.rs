//! One poll of the agent loop, phase by phase: [`prepare`] advances the
//! running slots, [`policy`] reads the disk declaration this tick admits
//! against, [`reconcile`] re-asserts the registry's host-level declarations,
//! and [`gates`] decides what the broadcast says and whether anything is
//! claimed at all.

pub mod gates;
pub mod policy;
pub mod prepare;
pub mod reconcile;

use std::time::{Duration, Instant};

use serde_json::{Map, Value};

use crate::providers::local::disk_cleanup;
use crate::providers::local::disk_staging;
use crate::providers::local::helpers;
use crate::providers::local::self_terminate;
use crate::providers::local::slots::ActiveSlot;
use crate::queue::capacity::CapacitySnapshot;
use crate::queue::JobStorage;
use crate::sizing::Sizing;

use reconcile::{GpuPowerLimitState, PlacementPolicyState};

use super::{agent_log, claim, Step, POLL_INTERVAL_S};

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
/// No global error handler wraps the loop body: unexpected errors
/// crash the agent visibly (returned as Err) so the operator can diagnose.
pub async fn run_agent(gpu_type: &str, idle_shutdown: bool, kind: &str) -> anyhow::Result<()> {
    let log_fn = &mut |msg: &str| agent_log(msg);

    let mut gpu_type = gpu_type.to_string();
    if gpu_type.is_empty() {
        gpu_type = helpers::detect_gpu_type().await;
    }
    let mut total_vram_gb = helpers::detect_local_vram_gb().await.max(1);
    log_fn(&format!(
        "Agent started. kind={kind} GPU={gpu_type} vram_gb={total_vram_gb} \
         cpu_cores={} capacity=live-resources",
        helpers::total_cpu_cores()
    ));
    disk_staging::setup_agent_staging(log_fn).await;

    let hostname = crate::providers::vast::system_hostname();
    log_fn("init: legacy workdir reaping disabled; cleanup is policy-owned");
    let initial_gpu = gpu_type.clone();

    let store = JobStorage::new().await?;
    log_fn("init: JobStorage done");
    let (storage_backend, store_answers_for_fleet) = prepare::bound_store(log_fn);
    let sizing = Sizing::new();
    let consumer_id = format!("{kind}-{hostname}");
    let mut slots: Vec<ActiveSlot> = Vec::new();
    let mut agent_diag: Map<String, Value> = Map::new();
    let fleet_staging = std::env::var("STADO_HF_FLUSH_STAGING_DIR")
        .ok()
        .filter(|path| !path.trim().is_empty());
    let mut last_fleet_flush = Instant::now();

    let mut last_cap: Option<CapacitySnapshot> = None;
    let mut gpu_power_limit_state: Option<GpuPowerLimitState> = None;
    let mut placement_policy_state: Option<PlacementPolicyState> = None;
    let mut pinned_only = false; // registry ComputeTarget.pinned_only, refreshed per poll
                                 // Python `disk_low_bytes = _persisted_disk_low_bytes()`: reuse the last
                                 // canonical low watermark from the janitor's owner-controlled state
                                 // file (cleanup may be unable to reach the registry during startup).
    let mut disk_low_bytes = disk_cleanup::persisted_disk_low_bytes();
    if disk_low_bytes.is_some() {
        log_fn("init: loaded validated disk low watermark from janitor state");
    }
    let (janitor_reports, _janitor) = prepare::spawn_janitor();
    // The broadcast keeps its declared cadence while the tick works. See
    // [`crate::providers::local::agent::heartbeat`] for why this is not a
    // liveness formality: it republishes only what the tick last measured, and
    // only while the tick is still starting iterations.
    let heartbeat = crate::providers::local::agent::heartbeat::CapacityHeartbeat::new();
    let _heartbeat = heartbeat.spawn(
        store.clone(),
        consumer_id.clone(),
        kind.to_string(),
        agent_log,
    );
    loop {
        let (tick_store_deadline, claim_store_deadline, vast_active) = prepare::advance_slots(
            &store,
            &sizing,
            &heartbeat,
            &janitor_reports,
            storage_backend,
            store_answers_for_fleet,
            &last_cap,
            &mut slots,
            &mut agent_diag,
            &mut disk_low_bytes,
            log_fn,
        )
        .await?;
        let (registry_target, current_free_bytes, pressure_active) = match policy::disk_policy(
            &store,
            &consumer_id,
            kind,
            &hostname,
            &fleet_staging,
            tick_store_deadline,
            total_vram_gb,
            &slots,
            &mut agent_diag,
            &mut disk_low_bytes,
            &mut last_cap,
            &mut last_fleet_flush,
            log_fn,
        )
        .await?
        {
            Step::Go(measured) => measured,
            Step::Done => continue,
            Step::Stop => return Ok(()),
        };
        match reconcile::registry_declarations(
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
        .await?
        {
            Step::Go(()) => {}
            Step::Done => continue,
            Step::Stop => return Ok(()),
        }
        let (mut free_vram_gb, mut cards) = match gates::inference::before_admission(
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
        .await?
        {
            Step::Go(measured) => measured,
            Step::Done => continue,
            Step::Stop => return Ok(()),
        };
        let (vram_buffer_gb, available_accelerators) = match gates::resources::measure(
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
        .await?
        {
            Step::Go(measured) => measured,
            Step::Done => continue,
            Step::Stop => return Ok(()),
        };
        match gates::admission::publish_and_admit(
            &store,
            &sizing,
            &consumer_id,
            kind,
            &gpu_type,
            total_vram_gb,
            free_vram_gb,
            idle_shutdown,
            pressure_active,
            claim_store_deadline,
            available_accelerators,
            &mut slots,
            &mut agent_diag,
            &mut last_cap,
            log_fn,
        )
        .await?
        {
            Step::Go(()) => {}
            Step::Done => continue,
            Step::Stop => return Ok(()),
        }
        let queued = match claim::queue::claimable(
            &store,
            &consumer_id,
            kind,
            &gpu_type,
            total_vram_gb,
            free_vram_gb,
            pinned_only,
            pressure_active,
            claim_store_deadline,
            current_free_bytes,
            disk_low_bytes,
            &slots,
            &mut agent_diag,
            log_fn,
        )
        .await?
        {
            Step::Go(queued) => queued,
            Step::Done => continue,
            Step::Stop => return Ok(()),
        };
        let started = claim::scan::claim_scan(
            &store,
            &sizing,
            &hostname,
            &consumer_id,
            kind,
            &gpu_type,
            total_vram_gb,
            pinned_only,
            vram_buffer_gb,
            claim_store_deadline,
            &queued,
            &cards,
            &last_cap,
            &mut slots,
            &mut free_vram_gb,
            &mut agent_diag,
            log_fn,
        )
        .await?;

        if started > 0 {
            continue;
        }

        if idle_shutdown
            && slots.is_empty()
            && helpers::no_eligible_in_queue(
                &store,
                &sizing,
                &gpu_type,
                total_vram_gb,
                free_vram_gb,
                kind,
                &consumer_id,
                slots.len(),
            )
            .await?
        {
            log_fn("idle_shutdown: no slots + no eligible queued jobs; exiting");
            self_terminate(kind, log_fn).await;
            return Ok(());
        }
        tokio::time::sleep(Duration::from_secs(POLL_INTERVAL_S)).await;
    }
}
