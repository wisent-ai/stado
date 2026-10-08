//! One claim scan: the budgets it opens with, the candidates it walks, and
//! the census it leaves behind in the diagnostics.

use chrono::Utc;
use serde_json::{Map, Value};

use crate::models::{activation_extraction_must_share_gpu, isoformat_utc, Job};
use crate::providers::local::helpers;
use crate::providers::local::slots::{ActiveSlot, CLAIM_DECLINED_KEY};
use crate::queue::capacity::CapacitySnapshot;
use crate::queue::JobStorage;
use crate::sizing::Sizing;

use super::fit::candidate_fit;
use super::start::start_candidate;

/// Walk what this tick may claim, start what still fits, and leave the census
/// of the scan in the diagnostics. Reports how many slots it started.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn claim_scan(
    store: &JobStorage,
    sizing: &Sizing,
    hostname: &str,
    consumer_id: &str,
    kind: &str,
    gpu_type: &str,
    total_vram_gb: i64,
    pinned_only: bool,
    queued: &[Job],
    cards: &[helpers::GpuCard],
    last_cap: &Option<CapacitySnapshot>,
    slots: &mut Vec<ActiveSlot>,
    free_vram_gb: &mut i64,
    agent_diag: &mut Map<String, Value>,
    log_fn: &mut dyn FnMut(&str),
) -> anyhow::Result<i64> {
    let mut started = 0i64;
    let mut diag_vram_rejected = 0i64;
    let mut diag_eligibility_rejected = 0i64;
    let mut diag_eligible = 0i64;
    let mut diag_claim_errors = 0i64;
    let mut diag_cpu_rejected = 0i64;
    let mut diag_ram_rejected = 0i64;
    let mut claim_declined = Vec::new();
    let mut available_cpu_cores = last_cap
        .as_ref()
        .map(|capacity| capacity.available_cpu_cores)
        .unwrap_or_default();
    // The free RAM this agent itself measured and broadcast with the capacity.
    let mut available_ram_gb = last_cap
        .as_ref()
        .and_then(|capacity| capacity.free_ram_gb)
        .unwrap_or_default();
    // These keys describe one completed scan. The previous scan was
    // already published before this point; carrying its last error into a
    // later clean scan would turn a historical refusal into current state.
    for key in [
        "last_claim_error_job",
        "last_claim_error",
        "last_claim_error_at",
    ] {
        agent_diag.remove(key);
    }
    // Per-card budget for this tick, emptiest first. The driver's frees
    // already include every allocation that exists; the subtraction below
    // covers the window in which a job this tick admitted has not allocated
    // yet, which is exactly when a second claim would otherwise be sized
    // against memory the first one is about to take.
    let mut card_budget: Vec<(String, i64)> = cards
        .iter()
        .map(|card| (card.uuid.clone(), card.free_vram_gb))
        .collect();
    for job in queued.iter() {
        let cmd = job.command.clone();
        let is_raw_share = activation_extraction_must_share_gpu(&cmd);
        let Some((need, requested_cpu_cores, requested_memory_gb)) = candidate_fit(
            store,
            sizing,
            job,
            &cmd,
            total_vram_gb,
            *free_vram_gb,
            available_cpu_cores,
            available_ram_gb,
            slots,
            agent_diag,
            &mut diag_cpu_rejected,
            &mut diag_ram_rejected,
            &mut diag_vram_rejected,
        )
        .await?
        else {
            continue;
        };
        if start_candidate(
            store,
            job,
            hostname,
            consumer_id,
            kind,
            gpu_type,
            total_vram_gb,
            pinned_only,
            need,
            requested_cpu_cores,
            requested_memory_gb,
            is_raw_share,
            &mut available_cpu_cores,
            &mut available_ram_gb,
            &mut card_budget,
            free_vram_gb,
            slots,
            agent_diag,
            &mut diag_eligibility_rejected,
            &mut diag_eligible,
            &mut diag_claim_errors,
            &mut claim_declined,
            &mut started,
            log_fn,
        )
        .await?
        {
            break;
        }
    }
    agent_diag.insert("queue_scanned".into(), Value::from(queued.len() as i64));
    agent_diag.insert("vram_rejected".into(), Value::from(diag_vram_rejected));
    agent_diag.insert("cpu_rejected".into(), Value::from(diag_cpu_rejected));
    agent_diag.insert("ram_rejected".into(), Value::from(diag_ram_rejected));
    agent_diag.insert(
        "eligibility_rejected".into(),
        Value::from(diag_eligibility_rejected),
    );
    agent_diag.insert("eligible_count".into(), Value::from(diag_eligible));
    agent_diag.insert(CLAIM_DECLINED_KEY.into(), Value::Array(claim_declined));
    agent_diag.insert("claimed_this_loop".into(), Value::from(started));
    agent_diag.insert("claim_errors".into(), Value::from(diag_claim_errors));
    agent_diag.insert(
        "last_claim_attempt_at".into(),
        Value::from(isoformat_utc(Utc::now())),
    );
    Ok(started)
}
