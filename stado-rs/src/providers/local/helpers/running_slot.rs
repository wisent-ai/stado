//! What one running slot is holding: exclusivity, VRAM, resident RAM, and
//! whether its process is still alive.
//!
//! Live measurement wins over any declared estimate here. A slot's numbers are
//! what admission subtracts from this host, so a stale pre-start guess would
//! let a second job in on memory the first one already took.

use std::path::Path;

use crate::config::{self, estimate_gpu_memory};
use crate::models::activation_extraction_must_share_gpu;
use crate::providers::local::helpers::host::ram::sum_rss_gb;
use crate::providers::local::helpers::MODEL_RE;
use crate::providers::local::{gpu_probe, Slot};
use crate::queue::{JobStorage, StorageError};
use crate::sizing::Sizing;

/// Python `_slot_is_exclusive`.
pub fn slot_is_exclusive(slot: &Slot) -> bool {
    if activation_extraction_must_share_gpu(&slot.job.command) {
        return false;
    }
    // Per-job opt-in: Job.exclusive=True takes precedence over the
    // regex-on-command path. Used for workloads (e.g. Z-Image LoRA
    // training, SDXL full finetune) whose peak VRAM is hard to bound from
    // the command string alone, but which the submitter has tagged
    // exclusive at submit time.
    if slot.job.exclusive {
        return true;
    }
    match MODEL_RE.captures(&slot.job.command) {
        Some(caps) => config::is_exclusive_model(caps[1].trim_matches(['\'', '"'])),
        None => false,
    }
}

/// Best known VRAM footprint for a running slot. Python `_slot_vram`.
///
/// Prefer live per-process nvidia-smi attribution when available. Fall
/// back to the declared/observed model estimate only before the job has
/// allocated CUDA memory. This keeps admission tied to measured live usage
/// instead of a stale pre-start estimate.
pub async fn slot_vram(
    slot: &Slot,
    sizing: &Sizing,
    store: &JobStorage,
) -> Result<i64, StorageError> {
    let declared = slot
        .job
        .gpu_mem_gb
        .max(estimate_gpu_memory(&slot.job.command, sizing, store).await?);
    let live = slot_live_vram_gb(slot).await;
    Ok(declared.max(live).max(slot.peak_vram_gb))
}

/// Python `_slot_live_vram_gb`: 0 when the slot has no pid or the probe is
/// unreadable (Python's broad `except Exception` / `max(0, ...)`).
pub async fn slot_live_vram_gb(slot: &Slot) -> i64 {
    let Some(pid) = slot.pid else { return 0 };
    gpu_probe::smi_job_used_gb(pid).await.max(0)
}

/// `kill(pid, 0)` liveness check, standing in for Python's
/// `proc.poll() is None`.
pub(crate) fn pid_alive(pid: i32) -> bool {
    nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None).is_ok()
}

/// True when a GPU slot is live but CUDA allocation is not visible yet.
/// Python `_slot_waiting_for_vram`.
pub async fn slot_waiting_for_vram(
    slot: &Slot,
    sizing: &Sizing,
    store: &JobStorage,
) -> Result<bool, StorageError> {
    let Some(pid) = slot.pid else {
        return Ok(false);
    };
    if !pid_alive(pid) {
        return Ok(false);
    }
    let declared = slot
        .job
        .gpu_mem_gb
        .max(estimate_gpu_memory(&slot.job.command, sizing, store).await?);
    Ok(declared > 0 && slot_live_vram_gb(slot).await <= 0)
}

/// Measured resident host RAM (GB) of a running slot's whole process tree
/// (bash + python + upload workers), summed from /proc/<pid>/status VmRSS.
/// This is the OBSERVED per-job footprint used to decide if another job
/// fits — no hardcoded estimate. Python `_slot_rss`.
pub async fn slot_rss(slot: &Slot) -> f64 {
    let Some(pid) = slot.pid else { return 0.0 };
    let pids = gpu_probe::proc_tree_pids(pid).await;
    sum_rss_gb(Path::new("/proc"), &pids)
}
