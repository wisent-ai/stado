//! One claim scan: the budgets it opens with, the candidates it walks, and
//! the census it leaves behind in the diagnostics.

use std::path::Path;
use std::time::Instant;

use chrono::Utc;
use serde_json::{Map, Value};

use crate::models::{activation_extraction_must_share_gpu, isoformat_utc, Job};
use crate::primitives::constants;
use crate::providers::local::disk::gate;
use crate::providers::local::helpers;
use crate::providers::local::slots::ActiveSlot;
use crate::queue::capacity::CapacitySnapshot;
use crate::queue::JobStorage;
use crate::sizing::Sizing;

use super::fit::candidate_fit;
use super::start::start_candidate;

/// Python `float(os.environ.get(key, default) or default)`.
fn env_f64(key: &str, default: f64) -> f64 {
    match std::env::var(key) {
        Ok(raw) if !raw.is_empty() => raw
            .trim()
            .parse()
            .unwrap_or_else(|_| panic!("{key} must be a float (Python float() parity): {raw}")),
        _ => default,
    }
}

/// Python `float(os.environ.get(primary, os.environ.get(fallback, d)) or d)`.
fn env_f64_chain(primary: &str, fallback: &str, default: f64) -> f64 {
    let parse = |key: &str, raw: String| {
        raw.trim()
            .parse()
            .unwrap_or_else(|_| panic!("{key} must be a float (Python float() parity): {raw}"))
    };
    match std::env::var(primary) {
        Ok(raw) if !raw.is_empty() => parse(primary, raw),
        Ok(_) => default,
        Err(_) => match std::env::var(fallback) {
            Ok(raw) if !raw.is_empty() => parse(fallback, raw),
            _ => default,
        },
    }
}

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
    vram_buffer_gb: i64,
    claim_store_deadline: Instant,
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
    let mut available_cpu_cores = last_cap
        .as_ref()
        .map(|capacity| capacity.available_cpu_cores)
        .unwrap_or_default();
    let mut available_ram_gb = last_cap
        .as_ref()
        .and_then(|capacity| {
            capacity.free_ram_gb.map(|free| {
                let reserve = capacity
                    .diag
                    .get("ram_safety_buffer_gb")
                    .and_then(Value::as_f64)
                    .unwrap_or(constants::RAM_SAFETY_BUFFER_MIN_GB as f64);
                (free - reserve).max(0.0)
            })
        })
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
    let raw_reserve = env_f64("WISENT_RAW_CLAIM_RESERVE_GB", 180.0);
    let raw_min_free = env_f64_chain(
        "WISENT_RAW_CLAIM_MIN_FREE_GB",
        "WISENT_RAW_HOT_FREE_TARGET_GB",
        270.0,
    );
    let tmpdir = std::env::var("TMPDIR").unwrap_or_else(|_| "/tmp".to_string());
    let raw_root = Path::new(&tmpdir).join("wisent_raw_pending");
    let raw_free = gate::free_gb(&raw_root);
    let mut raw_reserved = raw_reserve
        * slots
            .iter()
            .filter(|s| activation_extraction_must_share_gpu(&s.slot.job.command))
            .count() as f64;
    let mut diag_raw_disk_rejected = 0i64;
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
            is_raw_share,
            total_vram_gb,
            *free_vram_gb,
            available_cpu_cores,
            available_ram_gb,
            raw_free,
            raw_reserve,
            raw_reserved,
            raw_min_free,
            claim_store_deadline,
            slots,
            agent_diag,
            &mut diag_raw_disk_rejected,
            &mut diag_cpu_rejected,
            &mut diag_ram_rejected,
            &mut diag_vram_rejected,
            log_fn,
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
            vram_buffer_gb,
            need,
            requested_cpu_cores,
            requested_memory_gb,
            is_raw_share,
            raw_reserve,
            &mut available_cpu_cores,
            &mut available_ram_gb,
            &mut raw_reserved,
            &mut card_budget,
            free_vram_gb,
            slots,
            agent_diag,
            &mut diag_eligibility_rejected,
            &mut diag_eligible,
            &mut diag_claim_errors,
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
        "raw_disk_rejected".into(),
        Value::from(diag_raw_disk_rejected),
    );
    agent_diag.insert(
        "eligibility_rejected".into(),
        Value::from(diag_eligibility_rejected),
    );
    agent_diag.insert("eligible_count".into(), Value::from(diag_eligible));
    agent_diag.insert("claimed_this_loop".into(), Value::from(started));
    agent_diag.insert("claim_errors".into(), Value::from(diag_claim_errors));
    agent_diag.insert(
        "last_claim_attempt_at".into(),
        Value::from(isoformat_utc(Utc::now())),
    );
    Ok(started)
}
