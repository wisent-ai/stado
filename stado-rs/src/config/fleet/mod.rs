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

// --- Autonomous failure-fixer defaults ---
/// After this many fix attempts on the same job_id, the fixer stops
/// dispatching new Claude Code sessions so a permanently-broken job does
/// not burn unlimited subscription budget.
pub const FAILURE_FIXER_ATTEMPT_CAP: i64 = 3;
/// Per-job state-file prefix under BUCKET.
pub const FAILURE_FIXER_STATE_PREFIX: &str = "failure_fixes";

// --- Coverage verifier + retry orchestrator defaults ---
/// After this many submit attempts on the same group_key the orchestrator
/// marks the tuple UNFIXABLE and stops retrying.
pub const COVERAGE_ATTEMPT_CAP: i64 = 5;
/// Parallel verifier workers. Stays low to avoid HF rate-limit cap
/// (1000 requests / 300 s default).
pub const COVERAGE_VERIFY_THREADS: i64 = 4;
/// Stream progress every N entries during a verify walk.
pub const COVERAGE_PROGRESS_LOG_EVERY: i64 = 200;
/// GCS prefix under BUCKET for per-universe coverage state.
pub const COVERAGE_STATE_PREFIX: &str = "coverage";
