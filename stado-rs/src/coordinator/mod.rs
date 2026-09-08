//! Coordinator daemon — Rust port of `stado/coordinator.py`.
//!
//! The provider-neutral scheduling tick runs as a long-lived local process,
//! with no Cloud Run, Cloud Scheduler or Python control-plane dependency.
//!
//! The named coordinator entry supplies runtime cadence and identity only.
//! Queue/object state always comes from [`crate::queue::JobStorage::new`],
//! governed by Stado deployment config (`WC_STORAGE_BACKEND` plus its primary
//! and backup locators). A legacy registry `state_uri` is metadata and can
//! never override provider, backend, account, container or bucket.
//!
//! Cloud Function parity: `stado/cloud_function/main.py::monitor_jobs`
//! composes the SAME tick (fire due schedules -> normalize sizing ->
//! reap expired worker leases -> makespan assign -> per provider
//! check/reap/schedule -> run reaper -> billing collect) and needs no
//! separate port — [`run_tick`] is the single implementation. The lease
//! reaper is the tick's provider-neutral addition: it recovers `running/`
//! records whose worker died and queued pins naming silent workers, which
//! the per-cloud-provider monitor arms structurally cannot cover on a
//! local/box-only fleet. Credentials for both deployment shapes are resolved from
//! Skarbiec; the remaining deployment-specific difference is the box-owner
//! default (Cloud Function: "gcp-cloud-function"; daemon: hostname).
//!
//! Registry re-resolution runs every tick through
//! [`crate::targets::fetch_registry_remote`], which reads the configured
//! Stado store. The remote registry is the only authority for the
//! self-survival check — there is no local escape hatch.
//!
//! An unreadable primary (with no readable backup) is not interpreted as an
//! empty registry. The coordinator logs the storage failure and keeps
//! ticking; only a registry that was actually read may revoke the daemon.
//!
//! Release drift is resolved only from the exact configured Stado release
//! coordinate. No package-index channel participates in selection or update.
//! Billing: coordinator.py's daemon tick never collected billing (only
//!    the CF did). Per the port spec the tick includes the billing
//!    collector (fault-isolated, matching the CF), behind a flag so tests
//!    stay hermetic.
//!
//! The parts: `grant` resolves the scoped workload grant agent dispatch
//! projects into startup templates, `passes` holds the tick and the passes it
//! composes, and `daemon` is the loop that surrounds one tick. `log` and
//! `nodename` stay here because every part names them.

mod daemon;
mod grant;
mod passes;

pub(crate) use grant::{
    agent_workload_grant, secrets_from_skarbiec, AGENT_WORKLOAD_GRANT_B64,
    AZURE_AGENT_PROTECTED_GRANT,
};
pub(crate) use passes::run_autonomy_once;

pub use daemon::run;
pub use passes::{resolve_providers, run_tick, CoordinatorError, ResolvedProvider};

/// `[tick] ...` — the coordinator's log prefix (Python `_log`).
fn log(msg: &str) {
    eprintln!("[tick] {msg}");
}

/// `platform.node()` — the daemon-side default box-tick owner (Python
/// `os.uname().nodename`). Same approach as queue/submit.rs.
fn nodename() -> String {
    if let Ok(name) = std::env::var("HOSTNAME") {
        if !name.is_empty() {
            return name;
        }
    }
    std::process::Command::new("hostname")
        .output()
        .ok()
        .filter(|out| out.status.success())
        .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string())
        .filter(|name| !name.is_empty())
        .unwrap_or_default()
}
