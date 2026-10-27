//! The shared-mode hold one live workload keeps on the cleanup lock.

use std::fs::File;
use std::path::{Path, PathBuf};

use crate::providers::local::space_reclaim::janitor::pass::lock::file::{io_code, open_lock};
use crate::providers::local::space_reclaim::janitor::pass::lock::holds;
use crate::providers::local::space_reclaim::janitor::pass::lock::{
    ensure_state_dir, secure_home, user_home,
};
use crate::providers::local::space_reclaim::janitor::state::error::JanitorError;

/// The agent's word for "a standalone cleanup holds the exclusive lock, so
/// no workload can take its shared hold": published as `admission_reason`
/// in the capacity broadcast and as `space_reclaim_admission` beside a claim
/// that stopped for it, and read back verbatim by `stado host gates`.
pub const CLEANUP_IN_PROGRESS: &str = "cleanup_in_progress";

/// The agent's word for "the workload lock could not be probed at all":
/// the state directory or the lock file refused the open. A claim stops for
/// this exactly as it stops for [`CLEANUP_IN_PROGRESS`], so the publication
/// carries it as the `admission_reason`, with the error code appended under
/// `space_reclaim_admission`.
pub const CLEANUP_LOCK_ERROR: &str = "cleanup_lock_error";

/// A shared-mode hold on the cleanup lock for one live workload
/// (Python's opaque `int` handle from `acquire_workload_lock`), with the
/// record that names it to a janitor that finds the lock busy.
#[derive(Debug)]
pub struct WorkloadLock {
    file: Option<File>,
    record: Option<PathBuf>,
}

impl WorkloadLock {
    fn settle(&mut self) -> Option<File> {
        if let Some(record) = self.record.take() {
            if let Some(left) = crate::primitives::staging::remove(&record) {
                eprintln!("janitor: a released workload hold's record stays: {left}");
            }
        }
        self.file.take()
    }
}

impl Drop for WorkloadLock {
    fn drop(&mut self) {
        if let Some(file) = self.settle() {
            // Closing the file releases the hold too; a refused unlock is
            // said so the hold's end is not silent.
            if let Err(error) = file.unlock() {
                eprintln!("janitor: a workload hold's unlock was refused, and the hold ends when its file closes: {error}");
            }
        }
    }
}

/// Python `acquire_workload_lock` at an explicit home (test seam). `holder`
/// names the work the hold stands for (a job id) in the record a busy janitor
/// reads. While the janitor has asked for its turn, no new hold is given, as
/// while it holds the lock exclusively.
pub fn acquire_workload_lock_in(
    home: &Path,
    holder: &str,
) -> Result<Option<WorkloadLock>, JanitorError> {
    let state_dir = ensure_state_dir(&secure_home(home)?)?;
    if holds::turn_requested(&state_dir)? {
        return Ok(None);
    }
    let file = open_lock(&state_dir)?;
    match file.try_lock_shared() {
        Ok(()) => {
            let mut lock = WorkloadLock {
                file: Some(file),
                record: None,
            };
            lock.record = Some(holds::record(&state_dir, holder)?);
            Ok(Some(lock))
        }
        Err(std::fs::TryLockError::WouldBlock) => Ok(None),
        Err(std::fs::TryLockError::Error(error)) => Err(error.into()),
    }
}

/// Acquire the cleanup lock in shared mode for one live workload.
///
/// The returned opaque handle must be retained until the workload has
/// fully left its slot, then passed to [`release_workload_lock`]. `None`
/// means a standalone cleanup currently owns the exclusive lock, or has asked
/// for its turn, so admission must be retried later.
pub fn acquire_workload_lock(holder: &str) -> Result<Option<WorkloadLock>, JanitorError> {
    acquire_workload_lock_in(&user_home()?, holder)
}

/// The shared hold for one live workload that waits, as a lock wait in the
/// wait register, while a cleanup pass holds the lock exclusively, and takes
/// it the moment the pass lets go. Refused for a pass that is cleaning, an
/// installation would be refused again by the next one, so on a host whose
/// pressure never lifts it would never run. `None` only while the janitor has
/// asked for its turn: running workloads are letting it have the lock, and a
/// new hold would keep it from ever getting it.
pub fn wait_workload_lock(holder: &str) -> Result<Option<WorkloadLock>, JanitorError> {
    let state_dir = ensure_state_dir(&secure_home(&user_home()?)?)?;
    if holds::turn_requested(&state_dir)? {
        return Ok(None);
    }
    let file = open_lock(&state_dir)?;
    let path = state_dir.join(crate::providers::local::space_reclaim::janitor::LOCK_NAME);
    let what = format!(
        "the disk janitor's cleanup pass, for {holder} ({})",
        workload_holders()
    );
    match crate::wait::blocking_on_lock(what, &path, || file.lock_shared()) {
        Ok(Ok(())) => {}
        Ok(Err(error)) => return Err(error.into()),
        Err(cycle) => {
            return Err(JanitorError::os(&format!(
                "waiting for the cleanup lock {} would close a wait cycle: {cycle}",
                path.display()
            )))
        }
    }
    let mut lock = WorkloadLock {
        file: Some(file),
        record: None,
    };
    lock.record = Some(holds::record(&state_dir, holder)?);
    Ok(Some(lock))
}

/// Wait, as a lock wait in the wait register, until no workload holds the
/// cleanup lock: the moment the janitor's requested turn can be taken. The
/// lock is taken exclusively and let go at once, so the caller runs the pass
/// itself next; a turn request keeps new holds out in between, so nothing
/// takes the lock back before that pass does. This is how an operation asked
/// to wait for its turn waits, instead of being refused while the janitor
/// waits for a schedule that may not come before the operator does.
pub fn wait_for_janitor_turn(holder: &str) -> Result<(), JanitorError> {
    let state_dir = ensure_state_dir(&secure_home(&user_home()?)?)?;
    let file = open_lock(&state_dir)?;
    let path = state_dir.join(crate::providers::local::space_reclaim::janitor::LOCK_NAME);
    let what = format!(
        "the running workloads the disk janitor's turn waits on, for {holder} ({})",
        workload_holders()
    );
    match crate::wait::blocking_on_lock(what, &path, || file.lock()) {
        Ok(Ok(())) => {}
        Ok(Err(error)) => return Err(error.into()),
        Err(cycle) => {
            return Err(JanitorError::os(&format!(
                "waiting for the cleanup lock {} would close a wait cycle: {cycle}",
                path.display()
            )))
        }
    }
    file.unlock().map_err(JanitorError::from)
}

/// How many live workloads hold the cleanup lock now, as their records say:
/// the active job count a pass run outside the agent reports, in the type the
/// report carries it. Every pass outside the agent passes this, never a count
/// of its own choosing: a pass that reported none cleared caches under
/// running jobs.
pub fn live_workload_count() -> Result<i64, JanitorError> {
    let state_dir = ensure_state_dir(&secure_home(&user_home()?)?)?;
    let (workloads, unreadable) = holds::live(&state_dir);
    if let Some(problem) = unreadable.first() {
        return Err(JanitorError::os(&format!(
            "a workload hold record cannot be read, so the running workloads are not known: \
             {problem}"
        )));
    }
    i64::try_from(workloads.len()).map_err(|error| {
        JanitorError::os(&format!(
            "{} running workloads count past what a cleanup report holds: {error}",
            workloads.len()
        ))
    })
}

/// Who keeps the janitor from its turn on this machine: each live shared
/// hold with its acquisition timestamp, then the turn request and unread
/// records. This is a diagnostic observation, not permission to steal a lock.
pub fn workload_holders() -> String {
    let state_dir = match user_home()
        .and_then(|home| secure_home(&home))
        .and_then(|home| ensure_state_dir(&home))
    {
        Ok(state_dir) => state_dir,
        Err(error) => return format!("the cleanup lock's hold records cannot be read: {error}"),
    };
    let now = crate::providers::local::space_reclaim::janitor::state::report::build::epoch_now();
    let (holds, unread) = holds::live(&state_dir);
    let mut named: Vec<String> = holds
        .iter()
        .map(|hold| {
            format!(
                "{} (pid {}, acquired_at {} Unix seconds, held {}s)",
                hold.holder,
                hold.pid,
                hold.acquired_at,
                (now - hold.acquired_at).round()
            )
        })
        .collect();
    named.extend(unread);
    match holds::turn_request(&state_dir) {
        Ok(Some(turn)) => named.push(format!(
            "janitor turn requested_at {} Unix seconds, pid {:?}: {}",
            turn.requested_at, turn.pid, turn.reason
        )),
        Ok(None) => {}
        Err(error) => named.push(format!("cannot read janitor turn request: {error}")),
    }
    match open_lock(&state_dir) {
        Ok(file) => match super::file::read_lock_holder(&state_dir, &file) {
            Ok(Some(holder)) => named.push(format!(
                "recorded janitor {} {} (pid {}, acquired_at {} Unix seconds)",
                holder.writer, holder.writer_version, holder.pid, holder.acquired_at
            )),
            Ok(None) => {}
            Err(error) => named.push(error),
        },
        Err(error) => named.push(format!("cannot read cleanup lock: {error}")),
    }
    match named.is_empty() {
        true => "no holder or turn record identifies the refusal; ownership was not established"
            .to_string(),
        false => named.join("; "),
    }
}

/// Release a handle returned by [`acquire_workload_lock`]
/// (Python `release_workload_lock`: flock UN, then close; Drop provides the
/// same explicit unlock backstop when a caller releases by scope).
pub fn release_workload_lock(mut lock: WorkloadLock, log_fn: &mut dyn FnMut(&str)) {
    let Some(file) = lock.settle() else {
        return;
    };
    if let Err(exc) = file.unlock() {
        log_fn(&format!(
            "disk cleanup workload lock release failed: {}",
            io_code(&exc)
        ));
    }
}
