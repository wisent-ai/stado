//! The host jobs this process runs: starting a claimed job as a child shell,
//! recording the end of every child that exited, and failing a running record
//! no live worker holds.
//!
//! A host job runs as a fleet worker runs a job: in a directory of its own
//! (`~/.stado/host-store/work/<job-id>`), under its job identity
//! (`WC_JOB_ID`, `WC_JOB_STARTED_AT`, `STADO_BIN`), and what it leaves in that
//! directory's `output/` is kept in the host store under `status/<job-id>/output/`
//! when it ends, where `stado artifacts` against the host store reads it. A
//! routine that hands back its receipt as an artifact therefore works the same
//! from the host store as from the fleet store.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use chrono::{DateTime, Utc};

use crate::models::Job;
use crate::queue::runs::{COMPLETED, FAILED, RUNNING};
use crate::queue::JobStorage;

/// A job this process started, the child running it, and its directory.
pub(super) struct Running {
    pub(super) job: Job,
    pub(super) child: tokio::process::Child,
    work: PathBuf,
}

/// Claim `job` and start its command; `None` when a cancellation or another
/// claimant took it first.
pub(super) async fn start(
    store: &JobStorage,
    job: &Job,
    lease: chrono::Duration,
) -> Result<Option<Running>, String> {
    if !store
        .claim_queued_job(job, lease)
        .await
        .map_err(|error| format!("the claim failed: {error}"))?
    {
        return Ok(None);
    }
    let mut claimed = job.clone();
    claimed.started_at = Some(crate::models::isoformat_utc(Utc::now()));
    let root = super::root().map_err(|error| error.to_string())?;
    let logs = root.join("logs");
    let work = work_of(&job.job_id)?;
    let opened = std::fs::create_dir_all(&logs)
        .map_err(|error| format!("cannot create {}: {error}", logs.display()))
        .and_then(|()| {
            std::fs::create_dir_all(&work)
                .map_err(|error| format!("cannot create {}: {error}", work.display()))
        })
        .and_then(|()| {
            std::env::current_exe()
                .map_err(|error| format!("this Stado's own program cannot be named: {error}"))
        })
        .and_then(|stado| {
            let path = logs.join(format!("{}.log", job.job_id));
            let output = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
                .map_err(|error| format!("cannot open {}: {error}", path.display()))?;
            let errors = output
                .try_clone()
                .map_err(|error| format!("cannot share {} with stderr: {error}", path.display()))?;
            Ok((stado, output, errors))
        });
    let spawned = opened.and_then(|(stado, output, errors)| {
        let mut command = tokio::process::Command::new("/bin/sh");
        crate::providers::local::slots::apply_job_runtime_environment(
            &mut command,
            &claimed,
            &stado,
        );
        command
            .arg("-c")
            .arg(crate::providers::local::build_job_command(job))
            .current_dir(&work)
            .stdin(Stdio::null())
            .stdout(output)
            .stderr(errors)
            .spawn()
            .map_err(|error| format!("/bin/sh could not be started: {error}"))
    });
    match spawned {
        Ok(child) => Ok(Some(Running {
            job: claimed,
            child,
            work,
        })),
        Err(error) => {
            // Nothing ran in the tree, so there is nothing to keep from it.
            let error = match std::fs::remove_dir_all(&work) {
                Ok(()) => error,
                Err(left) if left.kind() == std::io::ErrorKind::NotFound => error,
                Err(left) => format!(
                    "{error}; its work tree {} was not removed: {left}",
                    work.display()
                ),
            };
            claimed.error = Some(error.clone());
            claimed.failed_at = Some(crate::models::isoformat_utc(Utc::now()));
            store
                .move_job(&claimed, RUNNING, FAILED)
                .await
                .map_err(|moved| {
                    format!("{error}; and the job was not moved to failed: {moved}")
                })?;
            Err(error)
        }
    }
}

/// Record the end of every child that exited.
pub(super) async fn settle(
    store: &JobStorage,
    running: &mut BTreeMap<String, Running>,
    log: fn(&str),
) {
    let mut ended = Vec::new();
    for (id, held) in running.iter_mut() {
        match held.child.try_wait() {
            Ok(Some(status)) => ended.push((id.clone(), Ok(status))),
            Ok(None) => {}
            Err(error) => ended.push((id.clone(), Err(error))),
        }
    }
    for (id, status) in ended {
        let Some(held) = running.remove(&id) else {
            continue;
        };
        let mut job = held.job;
        let now = crate::models::isoformat_utc(Utc::now());
        let kept = finish_work(store, &job, &held.work, log).await;
        let to = match (status, kept) {
            (Ok(status), Ok(())) if status.success() => {
                job.completed_at = Some(now);
                COMPLETED
            }
            (Ok(status), Ok(())) => {
                job.error = Some(format!("the command ended with {status}"));
                job.failed_at = Some(now);
                FAILED
            }
            (Ok(status), Err(cause)) => {
                job.error = Some(format!("the command ended with {status}, but {cause}"));
                job.failed_at = Some(now);
                FAILED
            }
            (Err(error), kept) => {
                let mut cause = format!("the child's exit could not be read: {error}");
                if let Err(also) = kept {
                    cause.push_str(&format!("; {also}"));
                }
                job.error = Some(cause);
                job.failed_at = Some(now);
                FAILED
            }
        };
        match store.move_job(&job, RUNNING, to).await {
            Ok(()) => log(&format!("host routines: {id} {to}")),
            Err(error) => log(&format!(
                "host routines: {id} ended but was not moved to {to}: {error}"
            )),
        }
    }
}

/// Fail every running host job no child of this process runs once its lease
/// has run out. Such a record was left by a worker that stopped before it
/// recorded the job's end, and the host store has no other reaper: left in
/// place, an overlap-skip schedule would read it as still running and never
/// fire again. A lease that has not run out may belong to a worker still
/// renewing it, so that record is left alone.
pub(super) async fn reap(store: &JobStorage, running: &BTreeMap<String, Running>, log: fn(&str)) {
    let held = match store.list_jobs(RUNNING).await {
        Ok(held) => held,
        Err(error) => {
            log(&format!(
                "host routines: listing running host jobs failed: {error}"
            ));
            return;
        }
    };
    let now = Utc::now();
    for mut job in held {
        if running.contains_key(&job.job_id) {
            continue;
        }
        let cause = match job.lease_expires_at.as_deref() {
            None => String::from("it states no lease, so no worker holds it"),
            Some(lease) => match DateTime::parse_from_rfc3339(lease) {
                Ok(expiry) if expiry <= now => format!(
                    "the worker that ran it stopped before recording its end; its lease ran \
                     out at {lease}"
                ),
                Ok(_) => continue,
                Err(error) => {
                    log(&format!(
                        "host routines: {} has a lease {lease:?} that is not a time ({error}); \
                         it is left running",
                        job.job_id
                    ));
                    continue;
                }
            },
        };
        // What the stopped worker's job left is kept and its tree removed,
        // as for a job whose end this process saw.
        let mut cause = cause;
        let finished = match work_of(&job.job_id) {
            Ok(work) => finish_work(store, &job, &work, log).await,
            Err(error) => Err(format!("its work tree cannot be named: {error}")),
        };
        if let Err(also) = finished {
            cause.push_str(&format!("; {also}"));
        }
        job.error = Some(cause.clone());
        job.failed_at = Some(crate::models::isoformat_utc(now));
        match store.move_job(&job, RUNNING, FAILED).await {
            Ok(()) => log(&format!("host routines: {} failed: {cause}", job.job_id)),
            Err(error) => log(&format!(
                "host routines: {} could not be moved to failed: {error}",
                job.job_id
            )),
        }
    }
}

/// The directory a host job runs in.
fn work_of(job_id: &str) -> Result<PathBuf, String> {
    Ok(super::root()
        .map_err(|error| error.to_string())?
        .join("work")
        .join(job_id))
}

/// Keep what an ended job left in its work tree's `output/`, then remove the
/// tree, so the host store holds no tree per fired routine. A tree whose
/// output could not be kept stays for inspection and the answer says where;
/// a tree that could not be removed is said in the log, its output already
/// kept. A job whose tree was never made (it ended before its start made it)
/// has nothing to keep.
async fn finish_work(
    store: &JobStorage,
    job: &Job,
    work: &Path,
    log: fn(&str),
) -> Result<(), String> {
    if !crate::primitives::presence::present(work).map_err(|error| error.to_string())? {
        return Ok(());
    }
    let output = work.join("output");
    crate::providers::local::slots::upload_output(store, job, &output)
        .await
        .map_err(|error| {
            format!(
                "its output in {} was not kept ({error}); its work tree stays there for inspection",
                output.display()
            )
        })?;
    if let Err(error) = std::fs::remove_dir_all(work) {
        log(&format!(
            "host routines: {}: its output is kept, but its work tree {} was not removed: {error}",
            job.job_id,
            work.display()
        ));
    }
    Ok(())
}
