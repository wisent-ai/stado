//! Who made a pass, and the entry points that start one.

use serde_json::Value;

use crate::providers::local::disk_cleanup::janitor::pass::once::cleanup_once;

/// Which process made a pass.
///
/// The state file has more than one writer on an always-on host: the queue
/// agent runs a pass every tick, and a `disk-cleanup --watch` unit runs one on
/// its own timer. [`crate::deploy::host_gates`] already documents what that
/// costs — it read a `low watermark 20 GiB, target 18 GiB` from a stale policy
/// alternating with the canonical 15/18 between one reading and the next — and
/// solved it for watermarks by preferring the registry declaration.
///
/// An `outcome` cannot be solved that way, because it is an event and not a
/// declaration. On 2026-08-31 the agent's pass at 14:55:24Z reported
/// `interval_noop` with no errors and all six cleaners scanned, and 46 seconds
/// later `stado space report` read `invalid_or_unavailable_policy` from the same
/// path: two processes, opposite verdicts, and the operator's answer decided by
/// which wrote last. A long-running writer holding a superseded configuration —
/// or an older binary that rejects a cleaner the registry now declares, which
/// makes it reject the whole document and resolve no policy at all — loses
/// nothing by overwriting a healthy report.
///
/// So every pass now says who made it and with which version. That does not
/// arbitrate between writers, and deliberately so: the file is the last pass by
/// whoever made it, which is a true thing to be. What changes is that a reader
/// can say so instead of presenting one process's verdict as the host's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CleanupWriter {
    /// The queue agent's per-tick pass.
    AgentTick,
    /// `stado disk-cleanup`, whether `--once` or under a `--watch` unit.
    Cli,
}

impl CleanupWriter {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AgentTick => "agent-tick",
            Self::Cli => "disk-cleanup-cli",
        }
    }
}

/// Resolve canonical policy and execute at most one bounded cleanup pass.
/// Python `run_cleanup_once`. Never fails: every failure mode lands in
/// the returned report (the agent mirrors Python's outcome handling).
pub async fn run_cleanup_once(
    active_job_count: i64,
    force: bool,
    writer: CleanupWriter,
    log_fn: &mut dyn FnMut(&str),
) -> Value {
    cleanup_once(active_job_count, force, false, false, writer, log_fn).await
}

/// Execute one bounded enforcing pass toward the policy's declared target,
/// even when free space is already above its low watermark and no older
/// continuation intent survived. The normal policy caps and durable frontier
/// still bound the pass; this only supplies the explicit missing goal.
pub async fn run_cleanup_to_target_once(
    active_job_count: i64,
    writer: CleanupWriter,
    log_fn: &mut dyn FnMut(&str),
) -> Value {
    cleanup_once(active_job_count, true, false, true, writer, log_fn).await
}

/// Resolve canonical policy, plan one bounded pass, and delete NOTHING.
///
/// NO Python original: `stado/providers/local/disk/cleanup.py` has no
/// preview entry point. This is the same canonical policy, the same
/// exclusive lock, the same scanners and the same caps as
/// [`run_cleanup_once`] — the returned report's `eligible_items` and
/// `expected_bytes` per cleaner are what a real pass would remove right
/// now. Two differences, both documented at their site in
/// [`run_with_lock`]: an `enforce` policy is pinned down to the janitor's
/// own `report` mode for the duration, and no state is written.
///
/// The interval gate is bypassed, because an operator who asks what the
/// cleanup would delete must get an answer rather than `interval_noop`,
/// and the preview carries zero running jobs because it is not the worker.
///
/// `stado disk-cleanup --dry-run` runs this locally; the `registry_cleanup`
/// stage of `stado space reclaim TARGET --dry-run`
/// ([`crate::deploy::host_state::cleanup`]) runs it on the target whose filesystem is
/// being previewed.
pub async fn preview_cleanup_once(log_fn: &mut dyn FnMut(&str)) -> Value {
    // A preview persists nothing, so its writer identity never reaches the
    // file; it is recorded anyway so the returned report is self-describing.
    cleanup_once(
        i64::default(),
        true,
        true,
        false,
        CleanupWriter::Cli,
        log_fn,
    )
    .await
}
