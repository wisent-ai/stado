//! Local worker agent: polls the queue, runs jobs concurrently when measured
//! CPU, RAM, disk, and accelerator resources allow it, and cooperatively
//! yields lower-priority jobs for higher-priority queued work.
//!
//! The runtime contract is framework-neutral:
//!   * no Python package is imported before claim;
//!   * NVIDIA admission uses the native `nvidia-smi` driver interface;
//!   * optional Hugging Face staging runs only when both
//!     `STADO_HF_FLUSH_STAGING_DIR` and `STADO_HF_FLUSH_PYTHON` are set;
//!   * job-specific runtimes, libraries, and GPU framework checks belong to
//!     the submitted workload.
//!
//! Registry self-lookup uses the configured Stado storage backend with the
//! bundled registry only as the documented fallback. Local release drift
//! triggers exact-coordinate binary self-update and re-exec; cloud machines
//! self-terminate for provider-owned replacement. A registry GPU-type change
//! remains an explicit operator restart.
//!
//! The janitor-owned disk-cleanup engine IS ported ([`super::disk_cleanup`]):
//! this loop runs `run_cleanup_once` every tick, holds a shared workload lock
//! per running job, and refuses new jobs while disk pressure is unresolved.
//! One behavioral simplification: Python releases a finished job's workload
//! lock explicitly and logs release failures; the Rust port lets the
//! [`super::slots::ActiveSlot`]'s `Drop` close the lock file (flock is
//! released on close), which cannot fail in a way worth logging.
//!
//! The phases live beside the loop: [`probes`] asks the host and the canonical
//! registry what is true, [`capacity`] turns that into a broadcast and an
//! eviction decision, [`tick`] runs one poll, and [`claim`] is the admission
//! scan that poll ends in.

pub mod capacity;
pub mod claim;
pub mod heartbeat;
pub mod janitor;
pub mod probes;
pub mod tick;

pub use capacity::yielding::{choose_yield_slots, maybe_yield_for_priority, YieldSlotInfo};
pub use probes::cuda::{cuda_probe_result, gpu_driver_available};
pub use probes::gpu_power::reconcile_gpu_power_limit;
pub use probes::registry::{load_registry_auto, lookup_auto, lookup_self_auto};
pub use tick::run_agent;

pub(crate) use probes::placement::reconcile_placement_policy;

use crate::primitives::constants;

/// Main agent poll interval (latency vs. storage-API load trade-off).
pub const POLL_INTERVAL_S: u64 = constants::POLL_INTERVAL_S;

/// Cooperative-yield anti-thrash floor: never evict a yieldable slot that has
/// run for less than this, so a just-(re)started background job gets real work
/// done before it can be bumped again. Pairs with Job.max_yields_before_protected.
pub const MIN_RUNTIME_BEFORE_YIELD_S: u64 = constants::MIN_RUNTIME_BEFORE_YIELD_S;

/// Cache TTL for the native NVIDIA driver-health probe.
pub const CUDA_PROBE_CACHE_S: u64 = constants::CUDA_PROBE_CACHE_S;

/// Claimable jobs one poll asks the queue for. It is a window over work this
/// agent may actually admit; CPU, RAM, VRAM, and disk budgets stop the scan.
const CLAIM_CANDIDATE_WINDOW: usize = 2_000;

/// Candidates the cooperative-yield scan considers before deciding what to
/// evict for.
const YIELD_CANDIDATE_WINDOW: usize = 200;

/// Job documents one scan may read while filling its window. Separating this
/// from the window is the whole point: a queue full of another host's work
/// costs scanning, and must not cost this host its candidates.
const QUEUE_SCAN_BUDGET: usize = 8_000;

/// What the poll loop does next once one of its phases has spoken.
///
/// The loop used to say this with bare `continue` and `return` statements
/// inside one function; the phases each own a piece of that function now, so
/// the same three answers are carried back rather than taken on the spot.
pub(crate) enum Step<T> {
    /// Carry on with this iteration, using what the phase measured.
    Go(T),
    /// The phase settled this tick; the loop starts the next one.
    Done,
    /// The agent leaves its loop cleanly.
    Stop,
}

/// Python `_log`: `[HH:MM:SS] [agent] msg` on stderr (local time).
pub fn agent_log(msg: &str) {
    let ts = chrono::Local::now().format("%H:%M:%S");
    eprintln!("[{ts}] [agent] {msg}");
}

/// Hard VRAM safety buffer at admission. The agent refuses to claim a
/// job if accepting it would leave less than this margin between
/// declared total VRAM use and the GPU's physical capacity. Catches the
/// class of failure where neighbor processes' actual peak exceeds their
/// declared gpu_mem_gb (estimate_gpu_memory has been observed to
/// under-call by 5-10 GB on 7-8B activation extraction workloads). The
/// buffer is independent of the per-job multipliers because it's the
/// LAST line of defense — if the per-job estimate is wrong, this catches
/// it before the n+1th job OOMs the entire VM.
/// Derived from total VRAM instead of a flat constant.
/// Python `_vram_safety_buffer_gb`.
pub fn vram_safety_buffer_gb(total_vram_gb: i64) -> i64 {
    (constants::VRAM_SAFETY_BUFFER_MIN_GB as i64)
        .max((total_vram_gb as f64 * constants::VRAM_SAFETY_BUFFER_FRACTION).ceil() as i64)
}

/// Tells the command wrapper to end the process so its declared supervisor can
/// start the installed Stado image. Ordinary loop errors remain retryable.
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub(crate) struct ReleaseHandoff(String);
