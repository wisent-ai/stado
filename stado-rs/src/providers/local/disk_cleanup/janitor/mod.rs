//! The janitor's own components.
//!
//! This module owns the fixed constants, paths and file-type
//! predicates every part of a pass shares; [`policy`] reads this host's
//! registry target and the fleet's declared releases, [`state`] owns the
//! state file and the report model, and [`pass`] owns the lock and the pass.

pub(crate) mod pass;
pub(crate) mod policy;
pub(crate) mod state;

pub(crate) const GIB: i64 = 1024 * 1024 * 1024;
/// Python `_STATE_VERSION`.
pub const STATE_VERSION: i64 = 1;

/// Per-writer attempt stamps in the state file: `{writer: epoch_seconds}`.
///
/// A KEY and not a version bump, deliberately: older binaries on the fleet
/// read the state file with `version == STATE_VERSION` exactly, and an
/// unknown key is ignored by them where a new version would make the whole
/// file unreadable.
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
/// Python `_MAX_ERRORS`.
pub(crate) const MAX_ERRORS: usize = 16;
/// The exclusive holder's identity and acquisition time, for diagnostics.
pub(crate) const LOCK_HOLDER_NAME: &str = "disk-cleanup.lock.holder";
/// Retired lock inodes remain linked under this prefix until their original
/// holder releases them. A replacement lock must never authorize deletion
/// while one of these files is still locked.
pub(crate) const RETIRED_LOCK_PREFIX: &str = "disk-cleanup.lock.retired.";
/// Inode-specific holder records survive a legacy predecessor removing the
/// canonical holder pathname after its lock inode has been retired.
pub(crate) const LOCK_HOLDER_INODE_PREFIX: &str = "disk-cleanup.lock.holder.inode.";

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
/// without ever naming the holder: a host can report them in alternation for
/// hours while every cleaner scans zero, and without this no command in the
/// fleet could say which process was holding it:
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
