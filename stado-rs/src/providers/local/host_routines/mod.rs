//! A host's own routines: schedules declared for this host alone, kept in a
//! store on this host, and fired and run by this host's own Stado process.
//!
//! A fleet schedule lives in the fleet store, is fired by the coordinator and
//! is run by whichever worker claims it, so every step crosses the store's
//! object API. A routine that keeps one host working (a supervisor pass, a
//! janitor) cannot depend on that: when the host that serves the fleet store
//! does not answer, nothing would be declared, fired or run. The host store is
//! a [`JobStorage`] over the local backend at `~/.stado/host-store`, with the
//! same schedule and job records as the fleet store, so the same schedule
//! code declares and fires them and the same records say what ran. Its pass
//! runs on the worker's poll and never waits on the fleet store.
//!
//! What the pass does each poll, in order: fire every due host schedule
//! ([`crate::schedules::fire_due_schedules`], overlap policy included), record
//! the sweep, record the end of every child that exited, fail every running
//! host job no child here runs whose lease has run out (a worker that stopped
//! mid-job), delete the cleaned sentinels of jobs that have settled, renew the
//! lease of every job this process still runs, and start every queued host
//! job as a
//! child shell running the job's command, its output appended to
//! `~/.stado/host-store/logs/<job>.log`. A child that exits moves its job to
//! `completed` or `failed`, the exit status in `error`.

mod jobs;

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;

use crate::capabilities::StorageAdapter;
use crate::queue::runs::{QUEUE, RUNNING};
use crate::queue::{JobStorage, LocalBackend, StorageError};

use jobs::{reap, settle, start, Running};

/// The directory of this host's own store, beside the rest of its Stado
/// state, under the home every Stado file is kept under
/// ([`crate::config_file::home`]), so a routine unit started without HOME
/// reads the same store as the operator's commands.
pub fn root() -> Result<PathBuf, StorageError> {
    let home = crate::config_file::home().map_err(|cause| {
        StorageError::Other(format!(
            "this host has no Stado directory for its host store: {cause}"
        ))
    })?;
    Ok(home.join(".stado").join("host-store"))
}

/// This host's own store, created on first use.
pub fn open() -> Result<JobStorage, StorageError> {
    let root = root()?;
    let path = root.to_str().ok_or_else(|| {
        StorageError::Other(format!(
            "the host store path {} is not UTF-8",
            root.display()
        ))
    })?;
    let backend = LocalBackend::new(path)?;
    Ok(JobStorage::with_backend_and_bucket(
        Arc::new(backend),
        StorageAdapter::Local.id(),
        path,
    ))
}

/// Fire, settle, renew and start once. Every failure is said in the log with
/// the step that failed; one step failing does not stop the others.
async fn pass(
    store: &JobStorage,
    running: &mut BTreeMap<String, Running>,
    lease: chrono::Duration,
    log: fn(&str),
) {
    let swept_at = Utc::now();
    let (fired, error) = match crate::schedules::fire_due_schedules(store, log, swept_at).await {
        Ok(fired) => (Some(fired), None),
        Err(error) => {
            log(&format!(
                "host routines: firing due schedules failed: {error}"
            ));
            (None, Some(error.to_string()))
        }
    };
    let sweep = crate::schedules::Sweep {
        at: crate::models::isoformat_utc(swept_at),
        host: crate::providers::vast::system_hostname(),
        fired,
        error,
    };
    if let Err(error) = crate::schedules::record_sweep(store, &sweep).await {
        log(&format!(
            "host routines: the sweep record was not written: {error}"
        ));
    }
    settle(store, running, log).await;
    reap(store, running, log).await;
    retire_sentinels(store, log).await;
    for held in running.values() {
        match store.renew_running_lease(&held.job.job_id, lease).await {
            Ok(true) => {}
            Ok(false) => log(&format!(
                "host routines: {} is no longer a running record; its child keeps running and \
                 its end will not be recorded",
                held.job.job_id
            )),
            Err(error) => log(&format!(
                "host routines: renewing the lease of {} failed: {error}",
                held.job.job_id
            )),
        }
    }
    let queued = match store.list_jobs(QUEUE).await {
        Ok(queued) => queued,
        Err(error) => {
            log(&format!(
                "host routines: listing queued host jobs failed: {error}"
            ));
            return;
        }
    };
    for job in queued {
        if running.contains_key(&job.job_id) {
            continue;
        }
        match start(store, &job, lease).await {
            Ok(Some(held)) => {
                log(&format!(
                    "host routines: started {} ({})",
                    job.job_id, job.command
                ));
                running.insert(job.job_id.clone(), held);
            }
            Ok(None) => {}
            Err(error) => log(&format!(
                "host routines: starting {} failed: {error}",
                job.job_id
            )),
        }
    }
}

/// Delete the cleaned sentinels every finished host job leaves in `queue/`
/// and `running/`, as the fleet reaper does for the fleet store
/// ([`JobStorage::retire_settled_sentinels`]: only a sentinel whose job has
/// settled in a terminal prefix). The host store has no reaper of its own,
/// so every routine fired every few minutes left two sentinels for good, and
/// each pass's `running/` and `queue/` listings downloaded all of them: a
/// laptop held 8,849 under `running/` after four days, the oldest from the
/// first day. A sweep that retired nothing is not said; one that failed, or
/// found objects that are not job records, is.
async fn retire_sentinels(store: &JobStorage, log: fn(&str)) {
    for prefix in [QUEUE, RUNNING] {
        match store.retire_settled_sentinels(prefix).await {
            Ok(sweep) => {
                if sweep.retired > 0 {
                    log(&format!(
                        "host routines: retired {} settled sentinel(s) under {prefix}/",
                        sweep.retired
                    ));
                }
                if !sweep.unreadable.is_empty() {
                    log(&format!(
                        "host routines: {prefix}/ holds {} object(s) that are not job records, \
                         left in place: {}",
                        sweep.unreadable.len(),
                        sweep.unreadable.join("; ")
                    ));
                }
            }
            Err(error) => log(&format!(
                "host routines: retiring the settled sentinels under {prefix}/ failed: {error}"
            )),
        }
    }
}

/// Run the host's routines every `poll`, for the life of this process.
///
/// A store that cannot be opened ends the runner with that error in the log,
/// since nothing it does could succeed.
pub async fn run(poll: Duration) {
    let log: fn(&str) = |message| println!("[host-routines] {message}");
    let store = match open() {
        Ok(store) => store,
        Err(error) => {
            log(&format!(
                "the host store cannot be opened, so no host routine runs: {error}"
            ));
            return;
        }
    };
    // A held job's lease reaches past the next pass, which renews it: one
    // period until that pass and one for the pass itself to run.
    let lease = match chrono::Duration::from_std(poll + poll) {
        Ok(lease) => lease,
        Err(error) => {
            log(&format!(
                "the poll period {poll:?} is beyond a lease's range: {error}"
            ));
            return;
        }
    };
    let mut running = BTreeMap::new();
    let mut ticker = tokio::time::interval(poll);
    loop {
        ticker.tick().await;
        pass(&store, &mut running, lease, log).await;
    }
}
