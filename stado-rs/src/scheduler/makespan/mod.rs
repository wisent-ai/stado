//! Makespan-minimizing job-to-agent matcher. Sorts queue by
//! (-priority, -runtime) (LPT in time, runtime from completed-job
//! history keyed by (model, task)), then assigns each job to the
//! eligible agent that finishes it earliest under a VRAM-concurrency
//! model. Writes assigned_to on the queue blob; agent-side enforcement
//! lives in providers/local/helpers/_job_eligible. No runtime guesses:
//! jobs without history AND without an explicit runtime_seconds_estimate
//! stay unassigned and the operator sees a log naming them. The runtime-
//! history machinery lives in [`history`] (split out to keep this module
//! under the 300-line cap in Python; kept split here for parity).
//!
//! The matcher itself is split the same way: `agents` builds the live-worker
//! projection and the shared runtime estimate, `matcher` makes the per-job
//! placement decision, and `assign` drives the tick and writes the results.
//!
//! Port of `stado/scheduler/makespan/__init__.py`.

mod agents;
mod assign;
pub mod history;
mod matcher;

pub use agents::{AgentInfo, HEARTBEAT_TTL_S};
pub use assign::{assign_jobs, assign_jobs_at, repair_conflicting_pinned_assignments};

pub(crate) use agents::download_many;
