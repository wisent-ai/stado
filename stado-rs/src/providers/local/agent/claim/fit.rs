//! Whether one queued candidate fits the budgets this tick measured: raw
//! staging disk, CPU, RAM, and both VRAM safety margins.

use chrono::Utc;
use serde_json::{Map, Value};

use crate::config::estimate_gpu_memory;
use crate::models::{isoformat_utc, Job};
use crate::providers::local::agent::vram_safety_buffer_gb;
use crate::providers::local::helpers;
use crate::providers::local::slots::ActiveSlot;
use crate::queue::JobStorage;
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
    slots: &[ActiveSlot],
    agent_diag: &mut Map<String, Value>,
    diag_raw_disk_rejected: &mut i64,
    diag_cpu_rejected: &mut i64,
    diag_ram_rejected: &mut i64,
    diag_vram_rejected: &mut i64,
) -> anyhow::Result<Option<(i64, i64, f64)>> {
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
    // A submission that resolved to the CPU marker declared that it needs no
    // accelerator, and re-guessing from its command text overrides a fact
    // with a heuristic. A detached Jeden session carries its model route in
    // that text (`jeden run … --model openrouter/openrouter/free`), the
    // model-name scan read it as a GPU workload needing VRAM, and on a
    // laptop with one GiB of it every such session was refused silently, on
    // every poll, while the same session without `--model` ran.
    let need = if job.gpu_mem_gb == 0 && job.machine_type == crate::queue::submit::CPU_MACHINE_TYPE
    {
        0
    } else {
        job.gpu_mem_gb
            .max(estimate_gpu_memory(cmd, sizing, store).await?)
    };
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
    let mut projected_used = need;
    for s in slots {
        projected_used += helpers::slot_vram(&s.slot, sizing, store).await?;
    }
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
