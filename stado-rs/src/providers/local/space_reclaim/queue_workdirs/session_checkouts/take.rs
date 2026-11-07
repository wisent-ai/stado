//! Judging one session checkout and, in an enforcing pass, taking it: the
//! cheap reasons to keep it first (the host's ownership, the proof that a
//! session cloned it, a service or unit naming it), its size only after
//! them, then under the exclusive cleanup lock for that one checkout what
//! live processes hold and what git says only it has, and last one rename
//! aside and the removal, recorded with what went with it.

use std::path::Path;

use serde_json::Value;

use super::evidence::{self, Proof, SessionJob};
use super::{units, work, Pass, Place, CLEANER};
use crate::providers::local::space_reclaim::candidates::ledger;
use crate::providers::local::space_reclaim::holders::Holders;
use crate::providers::local::space_reclaim::janitor::pass::lock::file::{ExclusiveLock, LockState};
use crate::providers::local::space_reclaim::janitor::pass::lock::ownership::{
    acquire_lock_state, retired_locks_active,
};
use crate::providers::local::space_reclaim::janitor::policy::ownership;
use crate::providers::local::space_reclaim::janitor::state::report::{met::Met, CleanupReport};
use crate::providers::local::space_reclaim::weles::{
    aside_origin, aside_path, dir_size, remove_tree,
};
use crate::providers::local::space_reclaim::JanitorError;

/// What every checkout of one pass is judged against.
pub(super) struct Judged<'a> {
    /// The paths this host's registry service declarations name.
    pub declared: &'a [String],
    /// This host's `omp-session` records, when the queue was asked and
    /// answered.
    pub jobs: Option<&'a [SessionJob]>,
}

/// The proof that a fleet session cloned `checkout`: its clone marker, else
/// this host's queue record of the session that cloned it.
fn proof(
    home: &Path,
    checkout: &Path,
    jobs: Option<&[SessionJob]>,
) -> Result<Option<Proof>, String> {
    if let Some(marker) = evidence::marker(checkout) {
        return Ok(Some(marker));
    }
    match jobs {
        Some(jobs) => evidence::by_job(home, checkout, jobs),
        None => Ok(None),
    }
}

/// The exclusive cleanup lock for one tree, as every tree the janitor takes
/// whole is taken under: a workload's hold keeps it. `None` when it is not
/// this pass's, the reason counted with the tree's `size`.
fn cleanup_lock(
    pass: &Pass<'_>,
    size: u64,
    met: &mut Met,
    report: &mut CleanupReport,
) -> Option<ExclusiveLock> {
    let lock = match acquire_lock_state(pass.lock_dir, pass.writer) {
        Ok(LockState::Held(lock)) => lock,
        Ok(LockState::Busy { .. }) => {
            met.kept("cleanup_lock_busy", size);
            return None;
        }
        Err(error) => {
            report.add_error(CLEANER, &error);
            met.kept("cleanup_lock_unreadable", size);
            return None;
        }
    };
    match retired_locks_active(pass.lock_dir, &lock.file) {
        Ok(holders) if holders.is_empty() => Some(lock),
        Ok(_) => {
            met.kept("lock_predecessor_active", size);
            None
        }
        Err(error) => {
            report.add_error(CLEANER, &error);
            met.kept("lock_predecessor_unreadable", size);
            None
        }
    }
}

/// Remove `aside`, the checkout `original` moved aside, and record it with
/// `went`, what went with it ([`work::went_with`]), when that was read.
fn remove(
    pass: &Pass<'_>,
    aside: &Path,
    original: &Path,
    bytes: i64,
    went: Option<Value>,
    met: &mut Met,
    report: &mut CleanupReport,
) {
    match remove_tree(aside) {
        Ok(()) => {
            met.removed(Some(bytes.unsigned_abs()));
            let (lock_dir, writer) = (pass.lock_dir, pass.writer);
            let recorded = match went {
                Some(went) => {
                    ledger::record_tree_with(lock_dir, CLEANER, writer, original, bytes, went)
                }
                None => ledger::record_tree(lock_dir, CLEANER, writer, original, bytes, bytes),
            };
            if let Err(error) = recorded {
                report.add_error("removal_record", &error);
            }
        }
        Err(error) => {
            met.skipped("deletion_refused");
            report.add_error(
                CLEANER,
                &JanitorError::os(&format!(
                    "cannot remove {}, the session checkout moved aside from {}: {error}; the \
                     next pass removes it again",
                    aside.display(),
                    original.display()
                )),
            );
        }
    }
}

/// A checkout an earlier pass judged, moved aside and could not finish
/// removing: no session works in it any more, so it goes on every pass.
pub(super) fn remove_aside(
    pass: &Pass<'_>,
    aside: &Path,
    met: &mut Met,
    report: &mut CleanupReport,
) {
    let Some(original) = aside_origin(aside) else {
        return;
    };
    // `dir_size` sums lengths, so it is never negative.
    let bytes = dir_size(aside);
    let size = bytes.unsigned_abs();
    met.eligible(size);
    if !pass.enforcing {
        return;
    }
    let Some(lock) = cleanup_lock(pass, size, met, report) else {
        return;
    };
    remove(pass, aside, &original, bytes, None, met, report);
    drop(lock);
}

/// Judge one checkout and, in an enforcing pass, take it.
pub(super) fn take(
    pass: &Pass<'_>,
    judged: &Judged<'_>,
    checkout: &Path,
    place: Place,
    met: &mut Met,
    report: &mut CleanupReport,
) {
    if place == Place::Home && !ownership::may_delete(pass.deletable, checkout) {
        met.skipped(ownership::NOT_STADO_OWNED);
        return;
    }
    let proof = match proof(pass.home, checkout, judged.jobs) {
        Ok(Some(proof)) => proof,
        Ok(None) => {
            met.skipped("not_session_checkout");
            return;
        }
        Err(cause) => {
            met.skipped("clone_time_unread");
            report.add_error(
                CLEANER,
                &JanitorError::os(&format!("{} is kept: {cause}", checkout.display())),
            );
            return;
        }
    };
    if judged.declared.iter().any(|path| {
        let path = Path::new(path);
        path.starts_with(checkout) || checkout.starts_with(path)
    }) {
        met.skipped("declared_by_service");
        return;
    }
    match units::unit_naming(pass.home, checkout) {
        Ok(None) => {}
        Ok(Some(_unit)) => {
            met.skipped("named_by_unit");
            return;
        }
        Err(cause) => {
            met.skipped("unit_definitions_unread");
            report.add_error(
                CLEANER,
                &JanitorError::os(&format!(
                    "{} is kept: {cause}, so whether a service runs from it is not known",
                    checkout.display()
                )),
            );
            return;
        }
    }
    // Sized only once nothing cheaper keeps it. `dir_size` sums lengths, so
    // it is never negative.
    let bytes = dir_size(checkout);
    let size = bytes.unsigned_abs();
    if !pass.enforcing {
        if !work::holds_work(checkout, size, met, report) {
            met.eligible(size);
        }
        return;
    }
    let Some(lock) = cleanup_lock(pass, size, met, report) else {
        return;
    };
    // Read under the lock, for this checkout alone, and git asked after it:
    // a session that started after the pass began is seen, and so is a
    // commit it made, before anything is removed.
    let holders = match Holders::read() {
        Ok(holders) => holders,
        Err(cause) => {
            met.kept("process_table_unread", size);
            report.add_error(
                CLEANER,
                &JanitorError::os(&format!("{} is kept: {cause}", checkout.display())),
            );
            return;
        }
    };
    if holders.hold(checkout) {
        met.kept("held_by_process", size);
        return;
    }
    if work::holds_work(checkout, size, met, report) {
        return;
    }
    met.eligible(size);
    // Read before the rename, while the checkout is still where git knows it.
    let went = work::went_with(checkout);
    // One rename, right after the holders and git were read: a session that
    // starts from here on finds no checkout and clones a new one, and a
    // removal that fails half way leaves only the aside name.
    let aside = aside_path(checkout);
    if let Err(error) = std::fs::rename(checkout, &aside) {
        met.skipped("deletion_refused");
        report.add_error(
            CLEANER,
            &JanitorError::os(&format!(
                "cannot move {} ({}) aside to {} before removing it: {error}",
                checkout.display(),
                proof.describe(),
                aside.display()
            )),
        );
        return;
    }
    remove(pass, &aside, checkout, bytes, Some(went), met, report);
    drop(lock);
}
