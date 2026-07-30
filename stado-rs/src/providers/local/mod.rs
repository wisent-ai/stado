//! Local compute runtime: native GPU/VRAM detection, per-process measurement,
//! atomic queue claims, fenced slot lifecycle, disk admission and cleanup,
//! provider-owned machine termination, immutable release replacement, and the
//! resilient agent loop.
//!
//! The base runtime is framework-neutral. Optional Hugging Face rate limiting
//! and staging flush live in separate modules and activate only through their
//! explicit configuration. Shared command construction remains in
//! `scheduler::dispatch::box::output` and is re-exported here for the local
//! executor.

pub mod agent;
pub mod azure_self;
pub mod disk_cleanup;
pub mod disk_gate;
pub mod disk_staging;
pub mod fleet_flush;
pub mod gcp_self;
pub mod gpu_probe;
pub mod helpers;
pub mod hf_rate;
pub mod slots;
pub mod version_check;

pub use crate::scheduler::dispatch::r#box::output::{build_job_command, verify_command};

use crate::models::Job;

/// A running local-agent slot — the keys of Python local_agent's slot dict
/// that the helpers read (`job`, `proc.pid`, `peak_vram_gb`). The full
/// runtime slot (live child handle, log file, timestamps) is
/// [`slots::ActiveSlot`]; this struct stays the shared view the
/// [`helpers`] functions take. Liveness checks (`proc.poll() is None` in
/// Python) are done via `kill(pid, 0)` where needed.
#[derive(Debug, Clone)]
pub struct Slot {
    pub job: Job,
    /// Root pid of the job's `bash -c <cmd>` process; None before spawn.
    pub pid: Option<i32>,
    /// Measured peak GPU memory (GiB) observed over previous ticks.
    pub peak_vram_gb: i64,
}

impl Slot {
    pub fn new(job: Job, pid: Option<i32>) -> Self {
        Slot {
            job,
            pid,
            peak_vram_gb: 0,
        }
    }
}

/// Signal provider-neutral termination by ending the agent process.
///
/// The coordinator observes the disappeared capacity heartbeat and invokes
/// `Provider::delete_instance` through the owning adapter. Guest runtimes never
/// receive provider credentials or call provider deletion APIs, including on
/// idle shutdown and release drift.
pub async fn self_terminate(kind: &str, log_fn: &mut dyn FnMut(&str)) {
    log_fn(&format!(
        "termination requested for {kind}; scheduler/provider adapter cleanup owns the machine"
    ));
}
