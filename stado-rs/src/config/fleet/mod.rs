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

/// Defaults for the smart-routing CLI flags. 0 means "no cap"; the
/// scheduler only enforces a cost gate when this is positive.
pub const DEFAULT_MAX_COST_PER_HOUR_USD: f64 = 0.0;
pub const DEFAULT_PRIORITY: i64 = 0;
pub const DEFAULT_PREEMPTIBLE: bool = false;
pub const DEFAULT_ANY_PROVIDER: bool = true;

// --- Autonomous failure-fixer ---
/// Per-job state-file prefix under BUCKET.
pub const FAILURE_FIXER_STATE_PREFIX: &str = "failure_fixes";
