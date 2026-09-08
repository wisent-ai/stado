//! The live slot: the intervals and defaults it is measured against,
//! [`ActiveSlot`] (the workload's process group, log handle, capacity
//! accounting and shared janitor hold), and the [`SlotOutcome`] a tick
//! returns.

use super::*;

/// Write a fresh heartbeat every 60s; HEARTBEAT_STALE_MINUTES=15 leaves 15
/// missed-write tolerance. Python `HEARTBEAT_INTERVAL`.
pub const HEARTBEAT_INTERVAL_S: u64 = constants::SLOT_HEARTBEAT_INTERVAL_S;

/// Python `Job.yield_grace_seconds` fallback (`getattr(...) or 120`).
pub(crate) const DEFAULT_YIELD_GRACE_S: i64 = 120;
/// Python `Job.max_yields_before_protected` fallback (`getattr(...) or 5`).
pub const DEFAULT_MAX_YIELDS: i64 = 5;

/// A running local-agent slot (Python's slot dict). Owns the child process
/// handle; dropping it without reaping leaves the OS process running (the
/// child is in its own process group and re-parents to init), matching the
/// Python agent dropping a `Popen` handle.
pub struct ActiveSlot {
    /// The helper-visible slot state (`job`, `pid`, `peak_vram_gb`).
    pub slot: Slot,
    pub(crate) child: tokio::process::Child,
    /// Our copy of the log-file handle (stdout/stderr were dup'd from it).
    /// `None` after close — Python's flush+close-once discipline.
    pub(crate) log_file: Option<std::fs::File>,
    /// Last heartbeat stamp (monotonic). Python `last_hb` (time.time()).
    pub last_hb: Instant,
    /// Whether a heartbeat or finalization observed the canonical tree absent
    /// or replaced by a non-directory. Retained so a later recreation cannot
    /// erase the terminal evidence.
    pub(crate) workdir_missing: bool,
    /// Currently SIGSTOPed because a Vast renter appeared.
    pub paused: bool,
    /// Spawn time (monotonic), for the MIN_RUNTIME_BEFORE_YIELD_S guard.
    pub started_mono: Instant,
    /// Detached heartbeat task (daemon-thread parity); exits when the pid dies.
    pub(crate) _hb_task: tokio::task::JoinHandle<()>,
    /// Shared hold on the janitor's cleanup lock for this live workload
    /// (Python `slot["disk_cleanup_lock"]`).
    ///
    /// Released by [`ActiveSlot::reap`] the instant the workload's process is
    /// observed to have exited — NOT when the slot is dropped. The slot
    /// outlives the process on purpose: finalization (artifact upload, status
    /// write, the `running` -> terminal move) is retried on later ticks and
    /// keeps the slot alive for as long as the store refuses. Tying the hold
    /// to the slot therefore tied a cross-process lock to an unbounded retry.
    /// See [`release_hold_for_exited_workload`].
    pub disk_cleanup_lock: Option<super::disk_cleanup::WorkloadLock>,
    /// Driver UUID of the board this job was placed on, when the host has one
    /// to choose. The agent reads it back to keep the next claim off a card it
    /// has already filled, and to keep deliberately GPU-sharing jobs together
    /// on one board.
    pub gpu_uuid: Option<String>,
}

impl std::fmt::Debug for ActiveSlot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ActiveSlot")
            .field("job_id", &self.slot.job.job_id)
            .field("pid", &self.slot.pid)
            .field("paused", &self.paused)
            .finish()
    }
}

impl ActiveSlot {
    /// Root pid of the job's `sh -c <cmd>` process; also the process-group
    /// id (spawned with `process_group(0)`).
    pub fn pid(&self) -> i32 {
        self.slot
            .pid
            .expect("ActiveSlot always has a pid after spawn")
    }

    /// Python `slot["log_file"].flush(); slot["log_file"].close()`. The
    /// agent keeps no userspace buffer on this file (the child's writes go
    /// straight to the dup'd fd), so closing our copy is the whole flush —
    /// what matters is that it happens BEFORE the output upload reads the
    /// file from disk (see the 2026-05-06 zero-byte-log incident below).
    pub(crate) fn close_log(&mut self) {
        self.log_file.take();
    }

    /// The workload's exit status, or `None` while its process is still
    /// executing — and, on the first observation of an exit, the release of
    /// the janitor's shared cleanup hold.
    ///
    /// One expression and not two statements, deliberately. The hold means
    /// "a workload process is live on this host"; this is the exact moment
    /// that stops being true, and every path in [`advance_slot`] below this
    /// point is finalization. A separate release statement is one refactor
    /// away from being skipped by a new early return, which is precisely how
    /// the leak below was introduced.
    pub fn reap(
        &mut self,
        log_fn: &mut dyn FnMut(&str),
    ) -> std::io::Result<Option<std::process::ExitStatus>> {
        let status = self.child.try_wait()?;
        if status.is_some() {
            release_hold_for_exited_workload(&mut self.disk_cleanup_lock, log_fn);
        }
        Ok(status)
    }
}

/// Release the janitor's shared cleanup hold because the workload it stands
/// for is no longer executing. Idempotent: a hold already released is a no-op.
///
/// # The defect this exists to make impossible
///
/// [`super::disk_cleanup::acquire_workload_lock_in`] takes a SHARED `fs2` hold
/// on `~/.cache/wisent-compute/disk-cleanup.lock` per live workload, and
/// `disk_cleanup`'s own pass needs that same file EXCLUSIVELY. One shared hold
/// that is never released therefore makes every exclusive acquire fail
/// forever — not for a while, forever — and a cleanup pass answers `lock_busy`
/// on every tick without ever scanning a directory.
///
/// The hold used to live and die with the [`ActiveSlot`], and a slot is
/// deliberately retained past its workload: when the terminal artifact upload
/// fails, [`advance_slot`] returns [`SlotOutcome::Running`] so finalization can
/// be retried on a later tick. That retry is unbounded by construction. So a
/// store that keeps refusing one upload converted a cross-process lock into a
/// permanent one, on a process that was otherwise perfectly healthy and kept
/// publishing capacity throughout.
///
/// Measured on `charless-mac-mini` on 2026-09-03: the agent (pid 79473, alive
/// 11.5 hours) held the lock, `space report` named it as the holder, every pass
/// reported `outcome: lock_busy, duration_ms: 372`, and the janitor's last
/// success stayed at 16:40:29Z. `host gates` then read that success age
/// against `STALL_INTERVALS * 300s` and reported `disk_cleanup_stalled`, which
/// closed the host to all work — on 18.4 GiB free against a 15 GiB watermark,
/// with eight jobs pinned to it. `lukasz-macbook` was closed the same way on
/// the same day at 118.7 GiB free against 100. Those two are the whole of
/// `darwin-arm64` in the registry, so the platform had no builder at all.
///
/// And it could not clear itself. The agent replaces itself only once
/// `slots.is_empty()` (`agent::run_agent`'s release-handoff branch), which the
/// retained slot prevents; the wedge clears when a superseding release is
/// installed; and installing one needs a `darwin-arm64` builder, which is the
/// host this wedge closed.
pub fn release_hold_for_exited_workload(
    hold: &mut Option<super::disk_cleanup::WorkloadLock>,
    log_fn: &mut dyn FnMut(&str),
) {
    if let Some(hold) = hold.take() {
        super::disk_cleanup::release_workload_lock(hold, log_fn);
    }
}

/// The result of one [`advance_slot`] tick (Python's bool return: True =
/// still running).
// ActiveSlot is large (owns the Job + child handle), but the enum moves
// once per tick per slot — boxing would buy nothing measurable.
#[allow(clippy::large_enum_variant)]
pub enum SlotOutcome {
    Running(ActiveSlot),
    /// Completed, failed, OOM-escalated, or dropped as a duplicate.
    Done,
}
