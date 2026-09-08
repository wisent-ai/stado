//! The janitor's own components.
//!
//! This module owns the fixed constants, paths and file-type
//! predicates every part of a pass shares; [`policy`] resolves the
//! canonical policy and the watermarks, [`state`] owns the state file
//! and the report model, and [`pass`] owns the lock and the pass loop.

pub(crate) mod pass;
pub(crate) mod policy;
pub(crate) mod state;

use std::time::Duration;

pub(crate) const GIB: i64 = 1024 * 1024 * 1024;
/// Python `_STATE_VERSION`.
pub const STATE_VERSION: i64 = 1;

/// Per-writer attempt stamps in the state file: `{writer: epoch_seconds}`.
///
/// A KEY and not a version bump, deliberately. `persisted_disk_low_bytes_in`
/// requires `version == STATE_VERSION` exactly, and that value feeds
/// `disk_pressure_unresolved`, which fails admission CLOSED when the low
/// watermark is unknown. Bumping the version would therefore make every
/// binary older than this one treat the state file as unreadable and stop
/// admitting work, on a fleet that demonstrably runs several versions at
/// once. An unknown key is ignored by those readers instead.
pub(crate) const WRITER_ATTEMPTS: &str = "last_attempt_by_writer";
/// Python `_STATE_DIR` (`~/.cache/wisent-compute`).
pub(crate) const STATE_DIR_PARTS: [&str; 2] = [".cache", "wisent-compute"];
/// Python `_LOCK_NAME`.
pub(crate) const LOCK_NAME: &str = "disk-cleanup.lock";
/// Python `_STATE_NAME`.
pub(crate) const STATE_NAME: &str = "disk-cleanup-state.json";
/// Serializes the read/merge/rename state transaction. The run lock cannot
/// serve this purpose because prevented writers intentionally persist while
/// another process holds it.
pub(crate) const STATE_LOCK_NAME: &str = "disk-cleanup-state.lock";
/// Python `_DEADLINE_SECONDS`.
pub(crate) const DEADLINE_SECONDS: f64 = 30.0;
/// Python `_MAX_ERRORS`.
pub(crate) const MAX_ERRORS: usize = 16;
/// Who holds the exclusive run lock, and until when they said they would.
///
/// `flock` states that somebody holds the lock and can state nothing else.
/// That was enough while every holder finished, and on 2026-09-03 it was not:
/// `charless-mac-mini` reported `disk_cleanup_stalled` for nine and a half
/// hours because one agent process held this lock, idle at 0% CPU with
/// eleven ESTABLISHED sockets to an object API whose pid no longer existed,
/// and a hold that never ends disables cleanup on the host permanently. The
/// kernel frees a dead holder's lock; it cannot free a live holder that will
/// never come back, and nothing in the file said the holder was overdue.
pub(crate) const LOCK_HOLDER_NAME: &str = "disk-cleanup.lock.holder";
/// How long past a holder's own declared deadline the lock may be taken over.
///
/// The criterion is deliberately NOT elapsed time alone: a long pass on a
/// large tree is healthy, and stealing its lock would produce exactly the
/// concurrent deletion the lock exists to prevent. It is the holder's OWN
/// promise — the pass deadline it recorded when it acquired the lock — plus
/// this grace. A holder past that has either stopped or lied about its
/// budget, and both are states nobody should have to wait out.
pub(crate) const LOCK_TAKEOVER_GRACE_S: f64 = 300.0;
/// Retired lock inodes remain linked under this prefix until their original
/// holder releases them. A replacement lock must never authorize deletion
/// while one of these files is still locked.
pub(crate) const RETIRED_LOCK_PREFIX: &str = "disk-cleanup.lock.retired.";
/// Serializes the short compare-and-replace sequence between takeover
/// contenders. It is never held while a cleanup pass runs.
pub(crate) const TAKEOVER_LOCK_NAME: &str = "disk-cleanup.lock.takeover";
/// Inode-specific holder records survive a legacy predecessor removing the
/// canonical holder pathname after its lock inode has been retired.
pub(crate) const LOCK_HOLDER_INODE_PREFIX: &str = "disk-cleanup.lock.holder.inode.";

/// How long one pass may wait on the queue store for its workdir keep-list.
///
/// NO Python original. Every other bound in this module — [`DEADLINE_SECONDS`],
/// `max_scan_items`, `max_items_per_pass` — governs work done AFTER the lock,
/// and the keep-list read is the only thing a pass waits on before it. It had
/// no bound at all, and the store's own HTTP client sets no timeout either
/// (`queue::gcs` builds a bare `reqwest::Client`), so a stalled listing
/// stalled the pass for as long as the transport took. On 2026-09-03
/// charless-mac-mini published `duration_ms: 818021` for a pass that reached
/// no cleaner.
///
/// Half of [`DEADLINE_SECONDS`], because the whole point of the janitor's own
/// default pass budget is that a pass is a short thing, and a keep-list read
/// that outlasts the scan it feeds is not a slow read but a broken one. The
/// expiry is not a failure: `None` is the keep-list's modelled unreadable
/// answer and [`queue_workdirs`] already refuses to delete on it and records
/// `queue_store_unreadable`.
pub(crate) const KEEP_LIST_BUDGET: Duration = Duration::from_secs(DEADLINE_SECONDS as u64 / 2);

/// The janitor's state file relative to `$HOME` — `_STATE_DIR` joined with
/// `_STATE_NAME` in the Python original.
///
/// Exported because [`crate::deploy::host_disk`] reports the cleanup state
/// of a host it is not running on, and has to name the exact file
/// [`ensure_state_dir`] and `write_state` maintain. A second copy of that
/// path living in the deploy layer would be one rename away from silently
/// reporting "never ran" for a host that runs cleanly every minute.
pub fn state_relative_path() -> String {
    let mut parts: Vec<&str> = STATE_DIR_PARTS.to_vec();
    parts.push(STATE_NAME);
    parts.join("/")
}

/// The janitor's exclusive run lock relative to `$HOME`, exported for the
/// same reason as [`state_relative_path`].
///
/// `lock_busy` and the agent's `cleanup_in_progress` are two views of one
/// fact — somebody holds this file — and the product could print both
/// without ever naming the holder. On 2026-08-31 charless-mac-mini reported
/// them in alternation for hours while every cleaner scanned zero, and no
/// command in the fleet could say which process was holding it:
/// `host exec`'s allowlist has no form that names the owner of a file lock,
/// correctly, because an operator-supplied path there would be a hole. So
/// the path travels as a crate constant, and [`crate::deploy::host_disk`]
/// splices it into its own fixed remote program.
pub fn lock_relative_path() -> String {
    let mut parts: Vec<&str> = STATE_DIR_PARTS.to_vec();
    parts.push(LOCK_NAME);
    parts.join("/")
}

/// `st_mode & S_IFMT` (Python `stat.S_IFMT`); the mask value is identical
/// on every Unix the port targets.
pub(crate) fn ifmt(mode: u32) -> u32 {
    mode & 0o170000
}
pub(crate) const IFDIR: u32 = 0o040000;
pub(crate) const IFREG: u32 = 0o100000;
pub(crate) const IFLNK: u32 = 0o120000;
