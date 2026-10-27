//! Who made a pass, and the entry points that start one.

use std::time::Duration;

use serde_json::Value;

use crate::providers::local::space_reclaim::janitor::pass::once::cleanup_once;

/// Which process made a pass, and how often it makes one.
///
/// The state file has more than one writer on an always-on host: the queue
/// agent runs a pass every tick, and the host's standing health watch runs
/// one at its declared cadence. An
/// `outcome` is an event, so the file is the last pass by whoever made it,
/// and every pass says who made it and with which version: a reader can then
/// say so instead of presenting one process's verdict as the host's.
///
/// `every` is the writer's declared period. A writer that has one promises
/// its next pass in the state file; a single pass promises nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CleanupWriter {
    /// The queue agent's pass, once per poll.
    AgentTick { every: Duration },
    /// `stado space reclaim --local` or the resident `stado serve` role.
    /// A single pass promises no next run.
    Cli { every: Option<Duration> },
}

impl CleanupWriter {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AgentTick { .. } => "agent-tick",
            Self::Cli { .. } => "space-reclaim-cli",
        }
    }

    /// The period this writer makes passes at, when it makes more than one.
    pub fn every(self) -> Option<Duration> {
        match self {
            Self::AgentTick { every } => Some(every),
            Self::Cli { every } => every,
        }
    }
}

/// Apply the disk-full rule once. Never fails: every failure mode lands in
/// the returned report.
pub async fn run_cleanup_once(
    active_job_count: i64,
    writer: CleanupWriter,
    log_fn: &mut dyn FnMut(&str),
) -> Value {
    cleanup_once(active_job_count, false, writer, log_fn).await
}

/// Run every cleaner as a pass at the threshold would, and delete NOTHING.
///
/// The same lock and the same scanners as [`run_cleanup_once`] — the
/// returned report's `eligible_items` and `expected_bytes` per cleaner are
/// what a pass would remove if the volume were at the threshold now — and no
/// state is written. It carries the workloads running now, as a pass would,
/// so a preview on a busy host shows the caches a pass would keep for them
/// rather than promising to remove them.
///
/// `stado space reclaim --local --dry-run` runs this locally; the
/// `registry_cleanup` stage of a fleet preview runs it on its target.
pub async fn preview_cleanup_once(
    active_job_count: i64,
    log_fn: &mut dyn FnMut(&str),
) -> Value {
    // A preview persists nothing, so its writer identity never reaches the
    // file; it is recorded anyway so the returned report is self-describing.
    cleanup_once(active_job_count, true, CleanupWriter::Cli { every: None }, log_fn).await
}
