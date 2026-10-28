//! What one running slot is holding: exclusivity, VRAM, resident RAM, and
//! whether its process is still alive.
//!
//! Live measurement wins over any declared estimate here. A slot's numbers are
//! what admission subtracts from this host, so a stale pre-start guess would
//! let a second job in on memory the first one already took.

use crate::config::{self, estimate_gpu_memory};
use crate::primitives::procutil::pid_alive;
use crate::providers::local::probe::gpu;
use crate::providers::local::Slot;
use crate::queue::{JobStorage, StorageError};
use crate::sizing::Sizing;

/// Python `_slot_is_exclusive`.
pub fn slot_is_exclusive(slot: &Slot) -> bool {
    // Per-job opt-in: Job.exclusive=True takes precedence over the
    // regex-on-command path. Used for workloads (e.g. Z-Image LoRA
    // training, SDXL full finetune) whose peak VRAM is hard to bound from
    // the command string alone, but which the submitter has tagged
    // exclusive at submit time.
    if slot.job.exclusive {
        return true;
    }
    crate::sizing::model_of(&slot.job.command)
        .is_some_and(|model| config::is_exclusive_model(&model))
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
    let declared = estimate_gpu_memory(&slot.job.command, sizing, store)
        .await?
        .into_iter()
        .fold(slot.job.gpu_mem_gb, i64::max);
    let counted = declared.max(slot.peak_vram_gb);
    Ok(match slot_live_vram_gb(slot).await {
        Ok(Some(live)) => counted.max(live),
        Ok(None) => counted,
        // The declared and peak figures still bound the slot; the live one
        // is missing, and that is said rather than read as no use.
        Err(cause) => {
            eprintln!(
                "{}: live VRAM unread, admission counts its declared and peak figure: {cause}",
                slot.job.job_id
            );
            counted
        }
    })
}

/// What nvidia-smi attributes to the slot's process tree now; `None` when
/// the slot has no pid or its tree holds no GPU memory yet, and why when the
/// driver could not be read.
pub async fn slot_live_vram_gb(slot: &Slot) -> Result<Option<i64>, String> {
    let Some(pid) = slot.pid else {
        return Ok(None);
    };
    Ok(gpu::smi_job_used_gb(pid)
        .await?
        .filter(|used| used.is_positive()))
}

/// Whether any process of the group `pgid` leads still exists. A slot's
/// workload is spawned as its own process group, so its build or training
/// keeps the group alive after the launching shell exits; liveness keyed to
/// the shell's pid alone ended when the launcher did. `EPERM` is a group
/// whose processes belong to somebody else, which is still alive, as
/// [`crate::primitives::procutil::pid_alive`] answers for one pid.
pub(crate) fn group_alive(pgid: i32) -> bool {
    match nix::sys::signal::killpg(nix::unistd::Pid::from_raw(pgid), None) {
        Ok(()) => true,
        Err(errno) => errno == nix::errno::Errno::EPERM,
    }
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
    let declared = estimate_gpu_memory(&slot.job.command, sizing, store)
        .await?
        .into_iter()
        .fold(slot.job.gpu_mem_gb, i64::max);
    Ok(declared.is_positive()
        && slot_live_vram_gb(slot)
            .await
            .map_err(StorageError::Other)?
            .is_none())
}
