//! Whether one queued candidate fits the budgets this tick measured: raw
//! staging disk, CPU, RAM, and both VRAM safety margins.

use std::time::Instant;

use chrono::Utc;
use serde_json::{Map, Value};

use crate::config::estimate_gpu_memory;
use crate::models::{isoformat_utc, Job};
use crate::primitives::constants;
use crate::providers::local::agent::vram_safety_buffer_gb;
use crate::providers::local::helpers;
use crate::providers::local::slots::ActiveSlot;
use crate::queue::{JobStorage, StorageError};
use crate::sizing::Sizing;

/// Report `(need, requested cores, requested memory)` for a candidate this
/// tick can still afford, or `None` for one it has just refused.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn candidate_fit(
    store: &JobStorage,
    sizing: &Sizing,
    job: &Job,
    cmd: &str,
    is_raw_share: bool,
    total_vram_gb: i64,
    free_vram_gb: i64,
    available_cpu_cores: i64,
    available_ram_gb: f64,
    raw_free: f64,
    raw_reserve: f64,
    raw_reserved: f64,
    raw_min_free: f64,
    claim_store_deadline: Instant,
    slots: &[ActiveSlot],
    agent_diag: &mut Map<String, Value>,
    diag_raw_disk_rejected: &mut i64,
    diag_cpu_rejected: &mut i64,
    diag_ram_rejected: &mut i64,
    diag_vram_rejected: &mut i64,
    log_fn: &mut dyn FnMut(&str),
) -> anyhow::Result<Option<(i64, i64, f64)>> {
    let claim_budget_left = || claim_store_deadline.saturating_duration_since(Instant::now());
    if is_raw_share && raw_free >= 0.0 && raw_free - raw_reserved - raw_reserve < raw_min_free {
        *diag_raw_disk_rejected += 1;
        agent_diag.insert(
            "raw_claim_free_gb".into(),
            Value::from((raw_free * 10.0).round() / 10.0),
        );
        agent_diag.insert(
            "raw_claim_reserved_gb".into(),
            Value::from((raw_reserved * 10.0).round() / 10.0),
        );
        return Ok(None);
    }
    let requested_cpu_cores = helpers::requested_cpu_cores(job);
    if requested_cpu_cores > available_cpu_cores {
        *diag_cpu_rejected += 1;
        return Ok(None);
    }
    let requested_memory_gb = helpers::requested_memory_gb(job);
    if requested_memory_gb > available_ram_gb {
        *diag_ram_rejected += 1;
        return Ok(None);
    }
    // Sizing comes out of the store too. A lapsed budget skips THIS
    // candidate rather than falling back to the job's declared figure:
    // the declared figure is the floor, and admitting a job on it when
    // the measured estimate is unknown is how a host claims work that
    // does not fit.
    let Ok(estimated) =
        tokio::time::timeout(claim_budget_left(), estimate_gpu_memory(cmd, sizing, store)).await
    else {
        log_fn(&format!(
            "loop: VRAM estimate for {} exhausted this tick's {}s store budget; not claiming it this tick",
            job.job_id,
            constants::AGENT_CLAIM_STORE_BUDGET_S
        ));
        return Ok(None);
    };
    let need = job.gpu_mem_gb.max(estimated?);
    // Hard VRAM safety buffer: refuse if declared use after admission
    // would leave less than the dynamic VRAM safety buffer. Use live
    // free VRAM, not only slot-declared usage, so external users such
    // as ComfyUI are included in the post-claim margin.
    let claimable_vram_gb = (free_vram_gb - vram_safety_buffer_gb(total_vram_gb)).max(0);
    if need > claimable_vram_gb {
        *diag_vram_rejected += 1;
        agent_diag.insert(
            "last_buffer_reject_job_id".into(),
            Value::from(job.job_id.clone()),
        );
        agent_diag.insert(
            "last_buffer_reject_at".into(),
            Value::from(isoformat_utc(Utc::now())),
        );
        agent_diag.insert("last_buffer_reject_need_gb".into(), Value::from(need));
        agent_diag.insert(
            "last_buffer_reject_claimable_gb".into(),
            Value::from(claimable_vram_gb),
        );
        return Ok(None);
    }
    // Also retain the slot-declared projection as a backstop for
    // cases where nvidia-smi temporarily under-reports a starting
    // child process. Only meaningful when the job actually needs
    // VRAM: on sub-buffer hosts total-buffer goes negative, which
    // would otherwise reject even need==0 (CPU-only) jobs.
    // Same rule for the running slots' projection: one budget for the
    // whole projection, and an unfinished projection refuses the
    // candidate instead of admitting it against an incomplete total.
    let projection = tokio::time::timeout(claim_budget_left(), async {
        let mut projected_used = need;
        for s in slots {
            projected_used += helpers::slot_vram(&s.slot, sizing, store).await?;
        }
        Ok::<_, StorageError>(projected_used)
    })
    .await;
    let Ok(projected_used) = projection else {
        log_fn(&format!(
            "loop: running-slot VRAM projection exhausted this tick's {}s store budget; not claiming {} \
             this tick",
            constants::AGENT_CLAIM_STORE_BUDGET_S,
            job.job_id
        ));
        return Ok(None);
    };
    let projected_used = projected_used?;
    if need > 0 && projected_used > total_vram_gb - vram_safety_buffer_gb(total_vram_gb) {
        *diag_vram_rejected += 1;
        agent_diag.insert(
            "last_buffer_reject_job_id".into(),
            Value::from(job.job_id.clone()),
        );
        agent_diag.insert(
            "last_buffer_reject_at".into(),
            Value::from(isoformat_utc(Utc::now())),
        );
        return Ok(None);
    }
    Ok(Some((need, requested_cpu_cores, requested_memory_gb)))
}
