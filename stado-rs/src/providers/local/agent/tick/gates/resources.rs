//! The measured refusals: the disk gate, slots still settling for VRAM, the
//! NVIDIA driver probe, and the per-card accelerator figures the broadcast
//! offers.

use std::collections::BTreeMap;
use std::time::Duration;

use chrono::Utc;
use serde_json::{Map, Value};

use crate::models::isoformat_utc;
use crate::providers::local::agent::capacity::snapshot::{
    diag_map, measured_capacity, publish_branch,
};
use crate::providers::local::agent::{
    gpu_driver_available, vram_safety_buffer_gb, Step, POLL_INTERVAL_S,
};
use crate::providers::local::disk::gate;
use crate::providers::local::helpers;
use crate::providers::local::slots::ActiveSlot;
use crate::queue::capacity::CapacitySnapshot;
use crate::queue::JobStorage;
use crate::sizing::Sizing;

/// The VRAM safety buffer this tick admits against, and the accelerators its
/// broadcast offers.
pub(crate) type MeasuredOffer = (i64, BTreeMap<String, i64>);

/// Report `(VRAM safety buffer, accelerators this host offers)` once every
/// measured refusal has been applied to `free_vram_gb` and `cards`.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn measure(
    store: &JobStorage,
    sizing: &Sizing,
    consumer_id: &str,
    kind: &str,
    gpu_type: &str,
    total_vram_gb: i64,
    slots: &[ActiveSlot],
    free_vram_gb: &mut i64,
    cards: &mut Vec<helpers::GpuCard>,
    agent_diag: &mut Map<String, Value>,
    last_cap: &mut Option<CapacitySnapshot>,
    log_fn: &mut dyn FnMut(&str),
) -> anyhow::Result<Step<MeasuredOffer>> {
    let (refuse_disk, disk_diag) = gate::gate_and_maybe_evict(log_fn);
    agent_diag.extend(diag_map(&disk_diag));
    if refuse_disk {
        let snapshot = measured_capacity(
            slots,
            false,
            Some("disk_gate_refused"),
            BTreeMap::new(),
            0,
            total_vram_gb,
            agent_diag.clone(),
        );
        publish_branch(
            store,
            consumer_id,
            kind,
            "disk-gate-refused",
            &snapshot,
            log_fn,
        )
        .await?;
        *last_cap = Some(snapshot);
        tokio::time::sleep(Duration::from_secs(10)).await;
        return Ok(Step::Done);
    }
    let vram_buffer_gb = vram_safety_buffer_gb(total_vram_gb);
    let mut settling_ids: Vec<String> = Vec::new();
    for s in slots {
        if helpers::slot_waiting_for_vram(&s.slot, sizing, store).await? {
            settling_ids.push(s.slot.job.job_id.clone());
        }
    }
    if !settling_ids.is_empty() {
        agent_diag.insert(
            "settling_job_ids".into(),
            Value::Array(settling_ids.into_iter().map(Value::String).collect()),
        );
        let snapshot = measured_capacity(
            slots,
            false,
            Some("jobs_settling_for_vram"),
            BTreeMap::new(),
            0,
            total_vram_gb,
            agent_diag.clone(),
        );
        publish_branch(
            store,
            consumer_id,
            kind,
            "jobs-settling-for-vram",
            &snapshot,
            log_fn,
        )
        .await?;
        *last_cap = Some(snapshot);
        tokio::time::sleep(Duration::from_secs(POLL_INTERVAL_S)).await;
        return Ok(Step::Done);
    }
    if *free_vram_gb < vram_buffer_gb {
        // VRAM-tight host (apple-mps reports ~1GB): broadcast zero VRAM
        // capacity so the coordinator routes no VRAM work here, but keep
        // scanning the queue — jobs with need==0 (CPU-only: probierz
        // runs, smoke checks) stay claimable. The per-job VRAM checks
        // below still reject anything needing VRAM we don't have.
        agent_diag.insert("vram_buffer_gb".into(), Value::from(vram_buffer_gb));
        agent_diag.insert("vram_buffer_free_gb".into(), Value::from(*free_vram_gb));
    }
    if *free_vram_gb > 0 && slots.is_empty() && gpu_type.starts_with("nvidia") {
        let (cuda_ok, cuda_detail) = gpu_driver_available().await;
        agent_diag.insert("gpu_driver_ok".into(), Value::from(cuda_ok));
        agent_diag.insert("gpu_driver_detail".into(), Value::from(cuda_detail.clone()));
        agent_diag.insert(
            "gpu_driver_checked_at".into(),
            Value::from(isoformat_utc(Utc::now())),
        );
        if !cuda_ok {
            log_fn(&format!(
                "NVIDIA driver probe failed; GPU jobs disabled while CPU jobs remain eligible: {}",
                cuda_detail.chars().take(160).collect::<String>()
            ));
            *free_vram_gb = 0;
            cards.clear();
        }
    }
    // A policy refusal above sets `free_vram_gb` to 0 and falls through to
    // this publish, so it decides whether any card is offered at all; the
    // per-card frees decide how many.
    let broadcast_cards: Vec<i64> = if *free_vram_gb <= 0 {
        Vec::new()
    } else {
        cards.iter().map(|card| card.free_vram_gb).collect()
    };
    agent_diag.insert("gpu_cards".into(), Value::from(cards.len() as i64));
    agent_diag.insert(
        "gpu_free_vram_gb_per_card".into(),
        Value::from(
            cards
                .iter()
                .map(|card| Value::from(card.free_vram_gb))
                .collect::<Vec<_>>(),
        ),
    );
    let available_accelerators =
        helpers::build_capacity_dict_per_card(gpu_type, &broadcast_cards, total_vram_gb);
    Ok(Step::Go((vram_buffer_gb, available_accelerators)))
}
