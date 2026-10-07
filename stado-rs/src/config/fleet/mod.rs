//! Fleet-wide operational values: queue, capacity and reaper limits.

mod dashboard;
mod model;
mod release;
mod storage;

pub use dashboard::*;
pub use model::*;
pub use release::*;
pub use storage::*;

pub const INSTANCE_PREFIX: &str = "wisent";

/// The environment variable a rented container's startup script sets to the
/// name its agent publishes capacity under, because a container cannot set
/// its kernel hostname. A VM leaves it unset and is named by its hostname.
pub const WORKER_NAME_ENV: &str = "STADO_WORKER_NAME";

/// The name this agent publishes capacity under: `STADO_WORKER_NAME` when
/// the machine's startup script declared one, otherwise the kernel hostname.
pub fn worker_name() -> String {
    match std::env::var(WORKER_NAME_ENV) {
        Ok(name) if !name.trim().is_empty() => name.trim().to_string(),
        _ => crate::providers::vast::system_hostname(),
    }
}

/// Defaults for the smart-routing CLI flags. 0 means "no cap"; the
/// scheduler only enforces a cost gate when this is positive.
pub const DEFAULT_MAX_COST_PER_HOUR_USD: f64 = 0.0;
pub const DEFAULT_PRIORITY: i64 = 0;
pub const DEFAULT_PREEMPTIBLE: bool = false;
pub const DEFAULT_ANY_PROVIDER: bool = true;

// --- Autonomous failure-fixer ---
/// Per-job state-file prefix under BUCKET.
pub const FAILURE_FIXER_STATE_PREFIX: &str = "failure_fixes";
