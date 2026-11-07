//! A run lock held by running workloads: who they are, the pass's request
//! for its turn, and the cleaners that run anyway.
//!
//! A pass that met a workload hold used to return before any cleaner after
//! the lock ran, so while release jobs compiled the janitor freed nothing
//! but registered scratch: the pre-lock cleaners that remove under the run
//! lock (build caches, Cargo build directories, Xcode build data) only
//! measure and keep beside a hold. The cleaners whose areas no workload
//! writes in, or whose keep rules already protect what a running workload
//! uses, now run beside the holds; the cleaners only the exclusive hold
//! runs are named on every busy pass. At
//! the rule a pass that may delete asks new workloads to wait (see
//! [`describe_workloads`]); below it, or in a preview, it does not.

use std::fs::{File, TryLockError};
use std::path::Path;

use crate::providers::local::space_reclaim::janitor::pass::lock::file::{open_lock, open_lock_at};
use crate::providers::local::space_reclaim::janitor::pass::lock::holds;
use crate::providers::local::space_reclaim::janitor::policy::ownership;
use crate::providers::local::space_reclaim::janitor::state::error::JanitorError;
use crate::providers::local::space_reclaim::janitor::state::report::build::epoch_now;
use crate::providers::local::space_reclaim::janitor::state::report::CleanupReport;
use crate::providers::local::space_reclaim::janitor::{LOCK_NAME, UNHELD_LOCK_NAME};
use crate::providers::local::space_reclaim::{
    agent_logs, backup_twins, bin_backups, candidates, chromium_clones, compiler_cache,
    delivered_releases, docker, job_outputs, local_snapshots, object_evidence, package_caches,
    queue_workdirs, release_store, source_snapshots, weles,
};

/// The cleaners only the exclusive hold runs, as the report names them: a
/// workload can be writing in each of their areas (its work tree, its
/// outputs, the caches, containers, logs and source copies a running build
/// or agent uses), and the package caches are the second tier, taken only
/// after the first.
const WAIT_FOR_TURN: &[&str] = &[
    "huggingface_cache",
    weles::CLEANER,
    chromium_clones::CLEANER,
    queue_workdirs::CLEANER,
    job_outputs::CLEANER,
    backup_twins::CLEANER,
    release_store::CLEANER,
    object_evidence::CLEANER,
    compiler_cache::CLEANER,
    agent_logs::CLEANER,
    docker::CLEANER,
    source_snapshots::CLEANER,
    package_caches::CLEANER,
];

/// The cleaners [`run_unheld`] runs, in its order.
const UNHELD: &[&str] = &[
    bin_backups::CLEANER,
    delivered_releases::CLEANER,
    local_snapshots::CLEANER,
];

/// Take a lock without waiting: `Ok(None)` when another holder has it.
fn try_take(file: File, shared: bool) -> Result<Option<File>, JanitorError> {
    let taken = if shared {
        file.try_lock_shared()
    } else {
        file.try_lock()
    };
    match taken {
        Ok(()) => Ok(Some(file)),
        Err(TryLockError::WouldBlock) => Ok(None),
        Err(TryLockError::Error(error)) => Err(error.into()),
    }
}

/// Under workload holds, run the cleaners no running workload needs kept
/// whole: two whose keep rules already protect what it uses (an install's
/// newest backup and `.previous`, the installed and newest delivered
/// release), and Time Machine's local snapshots, which macOS writes and no
/// job reads, taken as an exclusive pass takes them: at the rule, on a host
/// whose whole home this pass may delete in. An
/// enforcing pass takes the run lock shared, so no exclusive pass starts
/// under it, and the unheld-cleaners lock exclusively, so no second pass
/// beside the holds runs them at once and one that meets it names it
/// ([`describe_unheld_pass`]); either refused, none runs, naming the lock.
/// A preview deletes nothing and takes neither. Removals go to the removal
/// record; every cleaner that did not run is named in `waited_cleaners`.
/// `persist` is `None` for a preview; `deletable` is the one tree a shared
/// host lets this pass delete in.
pub(super) fn run_unheld(
    home: &Path,
    state_dir: &Path,
    persist: Option<&Path>,
    deletable: Option<&Path>,
    report: &mut CleanupReport,
) {
    all_waiting(report);
    let enforcing = persist.is_some();
    let locks = (|| -> Result<Result<Vec<File>, &'static str>, JanitorError> {
        if !enforcing {
            return Ok(Ok(Vec::new()));
        }
        let mut held = Vec::new();
        for (path, name, shared) in [
            (None, LOCK_NAME, true),
            (Some(UNHELD_LOCK_NAME), UNHELD_LOCK_NAME, false),
        ] {
            let file = match path {
                None => open_lock(state_dir)?,
                Some(name) => open_lock_at(&state_dir.join(name))?,
            };
            match try_take(file, shared)? {
                Some(file) => held.push(file),
                None => return Ok(Err(name)),
            }
        }
        Ok(Ok(held))
    })();
    let _held = match locks {
        Ok(Ok(held)) => held,
        Ok(Err(refused)) => {
            let holder = if refused == LOCK_NAME {
                "another janitor pass holds it exclusively"
            } else {
                "another pass is running these cleaners beside the holds"
            };
            report.add_error(
                "unheld_cleaners",
                &JanitorError::blocking(&format!(
                    "{refused} refused this pass ({holder}), so it ran none of {}",
                    UNHELD.join(", ")
                )),
            );
            return;
        }
        Err(error) => {
            report.add_error("unheld_cleaners", &error);
            return;
        }
    };
    bin_backups::scan_bin_backups(home, enforcing, report);
    delivered_releases::scan_delivered_releases(home, enforcing, report);
    // The gate `run_cleaners` gives it: deleting, on a host whose whole home
    // is this pass's, at the rule; a shared host's are counted as not
    // Stado's, as that pass counts them.
    let snapshots = enforcing && deletable.is_none() && report.pressure_active == Some(true);
    local_snapshots::delete_all(home, snapshots, report);
    if enforcing && deletable.is_some() {
        *report
            .local_snapshots
            .skipped
            .entry(ownership::NOT_STADO_OWNED.to_string())
            .or_default() += 1;
    }
    report.waited_cleaners = WAIT_FOR_TURN.to_vec();
    if let Some(state_dir) = persist {
        if let Err(exc) = candidates::ledger::record_pass(state_dir, report) {
            report.add_error("removal_record", &exc);
        }
    }
}

/// Name every cleaner that runs after the lock as waiting: a busy pass ran
/// none of them unless [`run_unheld`] ran its own.
pub(super) fn all_waiting(report: &mut CleanupReport) {
    report.waited_cleaners = WAIT_FOR_TURN.iter().chain(UNHELD).copied().collect();
}

/// When no workload holds the run lock but a pass beside workload holds
/// still runs its cleaners under a shared hold, name that pass: the outcome
/// is `lock_busy`, another janitor pass, not an unattributed holder and not
/// a running job. Returns false when the unheld-cleaners lock is free.
pub(super) fn describe_unheld_pass(
    state_dir: &Path,
    report: &mut CleanupReport,
    log_fn: &mut dyn FnMut(&str),
) -> bool {
    let path = state_dir.join(UNHELD_LOCK_NAME);
    let busy = match open_lock_at(&path).and_then(|file| try_take(file, false)) {
        Ok(taken) => taken.is_none(),
        Err(mut error) => {
            // The one lock that would name the holder could not be read; the
            // unattributed outcome below carries why.
            error.message = format!("cannot probe {}: {}", path.display(), error.message);
            report.add_error("unheld_lock", &error);
            false
        }
    };
    if busy {
        let detail = format!(
            "held in shared mode by a janitor pass running {} beside workload holds",
            UNHELD.join(", ")
        );
        log_fn(&format!("disk cleanup: lock {detail}"));
        report.add_error("lock_busy", &JanitorError::os(&detail));
        report.outcome = "lock_busy".to_string();
    }
    busy
}

/// Name the running workloads that hold the lock in shared mode and count
/// them. While one live shared hold exists no exclusive hold can, so they are
/// the whole answer. When the volume is at the rule's threshold a pass that
/// may delete also asks for its turn, so no new workload takes a hold until a
/// pass has had the lock. Returns false when no live workload holds it.
pub(super) fn describe_workloads(
    state_dir: &Path,
    may_request_turn: bool,
    report: &mut CleanupReport,
    log_fn: &mut dyn FnMut(&str),
) -> bool {
    let (workloads, unreadable) = holds::live(state_dir);
    for problem in &unreadable {
        log_fn(&format!("disk cleanup: hold record unreadable: {problem}"));
        report.add_error("lock_holds", &JanitorError::os(problem));
    }
    if workloads.is_empty() {
        return false;
    }
    let now = epoch_now();
    let named = workloads
        .iter()
        .map(|hold| {
            format!(
                "{} (pid {}, acquired_at {} Unix seconds, held {:.0}s)",
                hold.holder,
                hold.pid,
                hold.acquired_at,
                (now - hold.acquired_at).max(0.0)
            )
        })
        .collect::<Vec<_>>()
        .join("; ");
    let mut detail = format!(
        "held in shared mode by {} running workload(s): {named}",
        workloads.len()
    );
    report.active_job_count = report.active_job_count.max(workloads.len() as i64);
    report.workload_holds = workloads;
    report.outcome = "lock_busy_workloads".to_string();
    if may_request_turn && report.pressure_active == Some(true) {
        match holds::request_turn(state_dir, &named) {
            Ok(()) => detail.push_str(
                "; the volume is at the disk-full threshold, so new workloads wait until a pass has run",
            ),
            Err(error) => report.add_error("cleanup_turn", &error),
        }
    }
    log_fn(&format!("disk cleanup: lock {detail}"));
    report.add_error("lock_busy", &JanitorError::os(&detail));
    true
}
