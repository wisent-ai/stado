//! Release drift, the Vast renter, and the inference reservation that may be
//! holding this host's GPU — everything that decides how much VRAM the claim
//! scan is allowed to see.

use std::collections::BTreeMap;
use std::time::Duration;

use serde_json::{Map, Value};

use crate::providers::local::agent::capacity::inference::{
    inference_container_running, queued_gpu_job_for_inference, set_inference_container_running,
};
use crate::providers::local::agent::capacity::snapshot::{
    diag_map, measured_capacity, publish_branch,
};
use crate::providers::local::agent::{Step, POLL_INTERVAL_S};
use crate::providers::local::disk_gate;
use crate::providers::local::helpers;
use crate::providers::local::slots::ActiveSlot;
use crate::providers::local::version_check::{self, DriftOutcome};
use crate::queue::capacity::CapacitySnapshot;
use crate::queue::JobStorage;
use crate::sizing::Sizing;

/// Report `(free VRAM, per-card frees)` for the rest of this tick, after the
/// release check and the inference reservation have both had their say.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn before_admission(
    store: &JobStorage,
    sizing: &Sizing,
    consumer_id: &str,
    kind: &str,
    gpu_type: &str,
    total_vram_gb: i64,
    pinned_only: bool,
    vast_active: bool,
    slots: &[ActiveSlot],
    agent_diag: &mut Map<String, Value>,
    last_cap: &mut Option<CapacitySnapshot>,
    log_fn: &mut dyn FnMut(&str),
) -> anyhow::Result<Step<(i64, Vec<helpers::GpuCard>)>> {
    // Cleanup already ran before the immutable release check. This gate is
    // admission/diagnostics-only and has no destructive side effects.
    let (_pre_refuse, pre_diag) = disk_gate::gate_and_maybe_evict(log_fn);
    agent_diag.extend(diag_map(&pre_diag));
    log_fn("loop: pre-drain release drift check");
    match version_check::maybe_drain_or_upgrade(!slots.is_empty(), log_fn, kind).await {
        // An update/re-exec failure was logged; keep claiming on the old
        // binary rather than wedging the fleet. A successful update
        // re-execs and never reaches this arm.
        DriftOutcome::Clean | DriftOutcome::DriftDetected => {}
        // Cloud replacement was requested through the provider adapter.
        DriftOutcome::SelfTerminated => return Ok(Step::Stop),
    }
    let inference_reservation = crate::inference::reservation::active();
    if vast_active {
        let snapshot = measured_capacity(
            slots,
            false,
            Some("vast_renter_active"),
            BTreeMap::new(),
            0,
            total_vram_gb,
            agent_diag.clone(),
        );
        publish_branch(
            store,
            consumer_id,
            kind,
            "vast-renter-active",
            &snapshot,
            log_fn,
        )
        .await?;
        *last_cap = Some(snapshot);
        tokio::time::sleep(Duration::from_secs(POLL_INTERVAL_S)).await;
        return Ok(Step::Done);
    }

    let mut used_vram = 0i64;
    for s in slots {
        used_vram += helpers::slot_vram(&s.slot, sizing, store).await?;
    }
    if slots.iter().any(|s| helpers::slot_is_exclusive(&s.slot)) {
        used_vram = total_vram_gb;
    }
    let mut free_vram_gb = (total_vram_gb - used_vram).max(0);
    // Every card, one driver read. `free_vram_gb` stays the answer to "will
    // one job fit", which is the emptiest board; the per-card list carries
    // the rest of the truth into the broadcast and into device selection,
    // because a host with two boards has two independent pools and the
    // pooled reading was wrong about both.
    let mut cards = helpers::smi_gpu_cards().await;
    cards.sort_by_key(|card| std::cmp::Reverse(card.free_vram_gb));
    let smi_free = cards
        .iter()
        .map(|card| card.free_vram_gb)
        .max()
        .unwrap_or(-1);
    if smi_free >= 0 && smi_free < free_vram_gb {
        free_vram_gb = smi_free;
    }
    if let Some(reservation) = &inference_reservation {
        agent_diag.insert(
            "inference_reservation".into(),
            Value::from(reservation.deployment.clone()),
        );
        agent_diag.insert(
            "inference_gpu_mode".into(),
            Value::from(reservation.gpu_mode.clone()),
        );
        if reservation.gpu_mode == crate::inference::schema::GPU_EXCLUSIVE {
            free_vram_gb = 0;
            log_fn(&format!(
                "exclusive inference reservation '{}': GPU claims disabled; CPU-only claims remain eligible",
                reservation.deployment
            ));
        } else {
            let queued_job = queued_gpu_job_for_inference(
                store,
                sizing,
                gpu_type,
                total_vram_gb,
                kind,
                consumer_id,
                slots.len(),
                pinned_only,
            )
            .await?;
            let gpu_work_active = used_vram > 0;
            let should_yield = gpu_work_active || queued_job.is_some();
            match inference_container_running(&reservation.deployment).await {
                Ok(true) if should_yield => {
                    let reason = queued_job
                        .as_ref()
                        .map(|(job_id, need)| format!("queued job {job_id} needs {need} GiB"))
                        .unwrap_or_else(|| "an admitted GPU job is active".to_string());
                    log_fn(&format!(
                        "yieldable inference '{}': pausing because {reason}",
                        reservation.deployment
                    ));
                    match set_inference_container_running(&reservation.deployment, false).await {
                        Ok(()) => return Ok(Step::Done),
                        Err(error) => {
                            log_fn(&format!(
                                "yieldable inference '{}': pause failed safely: {error}",
                                reservation.deployment
                            ));
                            free_vram_gb = 0;
                        }
                    }
                }
                Ok(true) => {
                    free_vram_gb = 0;
                    agent_diag.insert("inference_runtime_state".into(), Value::from("serving"));
                }
                Ok(false) if !should_yield && slots.is_empty() => {
                    agent_diag.insert("inference_runtime_state".into(), Value::from("resuming"));
                    let snapshot = measured_capacity(
                        slots,
                        false,
                        Some("inference_resuming"),
                        BTreeMap::new(),
                        0,
                        total_vram_gb,
                        agent_diag.clone(),
                    );
                    publish_branch(
                        store,
                        consumer_id,
                        kind,
                        "inference-resuming",
                        &snapshot,
                        log_fn,
                    )
                    .await?;
                    *last_cap = Some(snapshot);
                    log_fn(&format!(
                        "yieldable inference '{}': GPU queue drained; resuming service",
                        reservation.deployment
                    ));
                    if let Err(error) =
                        set_inference_container_running(&reservation.deployment, true).await
                    {
                        log_fn(&format!(
                            "yieldable inference '{}': resume failed: {error}",
                            reservation.deployment
                        ));
                    }
                    return Ok(Step::Done);
                }
                Ok(false) => {
                    agent_diag.insert("inference_runtime_state".into(), Value::from("yielded"));
                    if let Some((job_id, need)) = queued_job {
                        agent_diag.insert("inference_yield_for_job".into(), Value::from(job_id));
                        agent_diag.insert("inference_yield_for_vram_gb".into(), Value::from(need));
                    }
                }
                Err(error) => {
                    free_vram_gb = 0;
                    agent_diag.insert("inference_runtime_state".into(), Value::from("unknown"));
                    log_fn(&format!(
                        "yieldable inference '{}': runtime state unavailable; GPU claims disabled: {error}",
                        reservation.deployment
                    ));
                }
            }
        }
    }
    Ok(Step::Go((free_vram_gb, cards)))
}
