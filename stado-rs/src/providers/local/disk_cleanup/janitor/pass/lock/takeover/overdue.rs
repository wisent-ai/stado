//! Whether the hold on this lock is past its own declared deadline.
//!
//! A holder that wrote itself down is judged by the deadline it declared plus
//! a grace; a hold with no record at all is judged by how long the lock file
//! has been untouched, because that bounds the age of the hold that exists
//! right now. Either way a takeover happens only after a deadline has passed.

use std::fs::File;
use std::path::Path;

use crate::providers::local::disk_cleanup::janitor::pass::lock::file::{
    read_lock_holder, LockHolder,
};
use crate::providers::local::disk_cleanup::janitor::state::report::build::epoch_now;
use crate::providers::local::disk_cleanup::janitor::LOCK_TAKEOVER_GRACE_S;

fn overdue_holder(state_dir: &Path, lock: &File) -> Option<(LockHolder, f64)> {
    let holder = read_lock_holder(state_dir, lock)?;
    let overdue_seconds = epoch_now() - (holder.deadline_at + LOCK_TAKEOVER_GRACE_S);
    (overdue_seconds > 0.0).then_some((holder, overdue_seconds))
}

/// How long the lock file itself says the current hold has lasted.
///
/// The holder record is the precise answer and this is the observable one: the
/// canonical lock file is rewritten every time the lock changes hands, so its
/// modification time bounds the age of the hold that exists right now.
fn unattributed_overdue(lock: &File, pass_seconds: f64) -> Option<f64> {
    let modified = lock.metadata().ok()?.modified().ok()?;
    let age = std::time::SystemTime::now()
        .duration_since(modified)
        .ok()?
        .as_secs_f64();
    let allowance = pass_seconds + LOCK_TAKEOVER_GRACE_S;
    (age > allowance).then_some(age - allowance)
}

/// The predecessor this pass may take the lock from, recorded or not.
///
/// A hold with no holder record used to be permanent: `overdue_holder` reads
/// the record first and answers `None` without it, so the pass reported
/// `lock_busy_unattributed` and left. On `charless-mac-mini` on 2026-09-06
/// that state lasted two hours and counting - the agent's own janitor tick
/// held the kernel lock with no record, every later tick declined to take it,
/// and the host stopped reclaiming anything at 5.3 GiB free while fifty
/// queued documentation records waited for space that only this janitor
/// returns. A missing record is not a live budget: the lock file's own age is
/// the evidence that exists in every case, and a hold past the declared pass
/// deadline is overdue whether or not its owner wrote itself down.
pub(super) fn overdue_predecessor(
    state_dir: &Path,
    lock: &File,
    pass_seconds: f64,
) -> Option<(Option<LockHolder>, f64)> {
    if let Some((holder, overdue_seconds)) = overdue_holder(state_dir, lock) {
        return Some((Some(holder), overdue_seconds));
    }
    if read_lock_holder(state_dir, lock).is_some() {
        return None;
    }
    unattributed_overdue(lock, pass_seconds).map(|overdue_seconds| (None, overdue_seconds))
}
