//! Local-agent infrastructure: GPU/VRAM detection and per-slot measurement,
//! job eligibility, disk admission gates, staging redirection, GCE/Azure
//! self-awareness, PyPI version-drift detection, HF rate-limit token buckets,
//! the asynchronous fleet-staging flush, the per-slot lifecycle, and the
//! agent main loop.
//!
//! Port of `stado/providers/local/` + `stado/providers/local_agent.py`:
//!   helpers/__init__.py  -> [`helpers`]   (detection, eligibility, RAM gates)
//!   helpers/gpu_probe.py -> [`gpu_probe`] (per-job GPU-memory attribution)
//!   disk/gate.py         -> [`disk_gate`] (admission-only disk diagnostics)
//!   disk/cleanup.py      -> [`disk_cleanup`] (registry-authorized janitor)
//!   disk/staging.py      -> [`disk_staging`] (tmpfs /tmp TMPDIR redirect)
//!   gcp_self.py          -> [`gcp_self`]  (GCE metadata + self-terminate)
//!   (no Python source)   -> [`azure_self`] (Azure IMDS + self-delete; the
//!                           Azure half of what gcp_self does for GCE)
//!   version_check.py     -> [`version_check`] (PyPI drift; see deviations)
//!   hf_rate.py           -> [`hf_rate`]   (GCS-backed HF token buckets)
//!   fleet_flush.py       -> [`fleet_flush`] (rotation + detached flush spawn)
//!   local/slots.py       -> [`slots`]     (per-slot lifecycle)
//!   local_agent.py       -> [`agent`]     (run_agent main loop; see its
//!                           module docs for the deviations)
//!
//! `build_job_command` / `verify_command` / `repo_prelude` from
//! `providers/local/helpers/execution.py` were already ported alongside the
//! box dispatch runtime in `scheduler::dispatch::box::output` (see its module
//! docs); they are re-exported here so local-agent consumers have a single
//! import site without moving the code the box runtime depends on.

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

/// Python interpreter for subprocess probes/flushes (`import wisent` smoke
/// test, CUDA probe, fleet flush). Python's agent used `sys.executable` (the
/// venv interpreter); the Rust binary reads `$WC_PYTHON` first so launchd /
/// systemd units can point at the job environment's interpreter, and falls
/// back to `python3` on PATH.
pub fn python_bin() -> String {
    std::env::var("WC_PYTHON")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "python3".to_string())
}

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

/// Delete the cloud VM this agent is running on, so an ephemeral agent
/// that exits for want of work stops costing money. Best-effort: each
/// backend logs and swallows its own failures, and the caller exits
/// either way.
///
/// Dispatch is on the agent `kind` — the `--kind` consumer label the
/// startup templates pass ("gcp", "azure", "aws", "vast", or the default
/// "local") — rather than on a probe, so a workstation agent issues no
/// metadata request at all and a cloud agent issues only its own
/// provider's, never two link-local timeouts back to back. Labels with no
/// Azure counterpart keep the historical GCE probe, which self-guards
/// with [`gcp_self::on_gcp`].
pub async fn self_terminate(kind: &str, log_fn: &mut dyn FnMut(&str)) {
    match kind {
        // A physical box or a rented Vast.ai instance is not ours to
        // delete: Vast teardown is the fleet's `vast destroy`, and the
        // workstation must survive a misconfigured --idle-shutdown.
        "local" | "vast" => {}
        "azure" => azure_self::self_terminate(log_fn).await,
        _ => gcp_self::self_terminate(log_fn).await,
    }
}
