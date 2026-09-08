//! Fleet-wide operational values: queue, capacity and reaper limits.

mod dashboard;
mod model;
mod release;
mod storage;

pub use dashboard::*;
pub use model::*;
pub use release::*;
pub use storage::*;

pub const HEARTBEAT_STALE_MINUTES: i64 = 15;
pub const MAX_SCHEDULE_PER_TICK: i64 = 4;
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
/// Max characters of failed/<jid>.json error field included in the
/// dispatched fix prompt. Big enough for Claude to see the full stack.
pub const FAILURE_FIX_PROMPT_ERROR_BYTES: i64 = 4000;
/// Seconds between failure-fixer scan_and_dispatch iterations when the
/// LaunchAgent runs in tight loop.
pub const FAILURE_FIXER_TICK_SECONDS: i64 = 180;
/// Command substring the LaunchAgent passes to wc-fix scan-dispatch
/// --command-pattern. Empty string means scan every failed/ blob, which
/// exhausts Claude Code subscription quota fast (live failure 2026-05-22:
/// 273 dispatches in one tick burned the daily limit). Set this to the
/// workload the operator wants the autonomous fixer to target.
/// raw.extract_and_upload is the canonical activation extraction workload.
pub const FAILURE_FIXER_COMMAND_PATTERN: &str = "raw.extract_and_upload";
/// Max fully-terminal runs the by-run reaper deletes per coordinator tick.
/// Bounds per-tick GCS work so a large backlog drains over several ticks
/// instead of one multi-thousand-blob delete stalling the tick.
pub const RUN_REAP_PER_TICK: i64 = 50;
/// Queue blobs the priority-marker index repair examines per coordinator
/// tick. The sweep is the standing repair for a queued job whose marker
/// write did not land — an unindexed job is invisible to every scheduler —
/// so it runs forever with a wrapping cursor rather than latching complete.
/// Bounds per-tick work to a names-only listing plus this many bodies.
pub const MARKER_REPAIR_PER_TICK: usize = 500;

// --- Coverage verifier + retry orchestrator defaults ---
/// After this many submit attempts on the same group_key the orchestrator
/// marks the tuple UNFIXABLE and stops retrying.
pub const COVERAGE_ATTEMPT_CAP: i64 = 5;
/// HTTP 429 backoff base; sleep = COVERAGE_VERIFY_BACKOFF_BASE ** attempt.
pub const COVERAGE_VERIFY_BACKOFF_BASE: i64 = 2;
/// Parallel verifier workers. Stays low to avoid HF rate-limit cap
/// (1000 requests / 300 s default).
pub const COVERAGE_VERIFY_THREADS: i64 = 4;
/// Stream progress every N entries during a verify walk.
pub const COVERAGE_PROGRESS_LOG_EVERY: i64 = 200;
/// GCS prefix under BUCKET for per-universe coverage state.
pub const COVERAGE_STATE_PREFIX: &str = "coverage";
/// Max retry-loop iterations for verify_request before re-raising 429.
pub const COVERAGE_HTTP_RETRY_CAP: i64 = 8;
