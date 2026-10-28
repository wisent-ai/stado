//! The measured refusals: the disk gate, slots still settling for VRAM, the
//! NVIDIA driver probe, and the per-card accelerator figures the broadcast
//! offers.

use std::collections::BTreeMap;

use chrono::Utc;
use serde_json::{Map, Value};

use crate::models::isoformat_utc;
use crate::providers::local::agent::capacity::snapshot::{
    diag_map, measured_capacity, publish_branch,
};
use crate::providers::local::agent::{gpu_driver_available, Step};
use crate::providers::local::disk::gate;
use crate::providers::local::helpers;
use crate::providers::local::slots::ActiveSlot;
use crate::queue::capacity::CapacitySnapshot;
use crate::queue::JobStorage;
use crate::sizing::Sizing;

/// The accelerators this tick's broadcast offers.
pub(crate) type MeasuredOffer = BTreeMap<String, i64>;

/// Report the accelerators this host offers once every measured refusal has
/// been applied to `free_vram_gb` and `cards`.
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
    let (refuse_disk, disk_diag) = gate::measure_admission(log_fn);
    agent_diag.extend(diag_map(&disk_diag));
    if refuse_disk {
        let mut snapshot = measured_capacity(
            slots,
            false,
            Some("disk_gate_refused"),
            BTreeMap::new(),
            None,
            total_vram_gb,
            agent_diag.clone(),
        );
        if let Some(detail) = disk_diag.refusal_detail {
            snapshot
                .diag
                .insert("admission_detail".into(), Value::from(detail));
        }
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
        return Ok(Step::Done);
    }
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
            None,
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
        return Ok(Step::Done);
    }
    if free_vram_gb.is_positive() && slots.is_empty() && gpu_type.starts_with("nvidia") {
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
                cuda_detail
            ));
            // No card is offered, so no VRAM is: what is offered is the sum
            // of the offered cards' free VRAM.
            cards.clear();
            *free_vram_gb = cards.iter().map(|card| card.free_vram_gb).sum();
        }
    }
    // A policy refusal above leaves no card offered and falls through to
    // this publish, so it decides whether any card is offered at all; the
    // per-card frees decide how many.
    let broadcast_cards: Vec<i64> = if !free_vram_gb.is_positive() {
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
    Ok(Step::Go(available_accelerators))
}
