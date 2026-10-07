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

/// The poll period this process's agent was started with, handed to every job
/// it starts as `STADO_POLL_SECONDS` so a job that follows its own lifecycle
/// reads at the operator's cadence.
pub(crate) static POLL: std::sync::OnceLock<std::time::Duration> = std::sync::OnceLock::new();

// Claim and yield scans read every eligible queued job (JobScan want 0,
// scan_budget 0): CPU, RAM, VRAM and disk budgets decide what this agent
// admits, so no window or scan budget is chosen here.

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

/// Tells the command wrapper to end the process so its declared supervisor can
/// start the installed Stado image. Ordinary loop errors remain retryable.
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub(crate) struct ReleaseHandoff(String);
