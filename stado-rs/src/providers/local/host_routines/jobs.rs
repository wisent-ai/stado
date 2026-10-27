//! The host jobs this process runs: starting a claimed job as a child shell,
//! recording the end of every child that exited, and failing a running record
//! no live worker holds.

use std::collections::BTreeMap;
use std::process::Stdio;

use chrono::{DateTime, Utc};

use crate::models::Job;
use crate::queue::runs::{COMPLETED, FAILED, RUNNING};
use crate::queue::JobStorage;

/// A job this process started, and the child running it.
pub(super) struct Running {
    pub(super) job: Job,
    pub(super) child: tokio::process::Child,
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
    let logs = super::root()
        .map_err(|error| error.to_string())?
        .join("logs");
    let opened = std::fs::create_dir_all(&logs)
        .map_err(|error| format!("cannot create {}: {error}", logs.display()))
        .and_then(|()| {
            let path = logs.join(format!("{}.log", job.job_id));
            let output = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
                .map_err(|error| format!("cannot open {}: {error}", path.display()))?;
            let errors = output
                .try_clone()
                .map_err(|error| format!("cannot share {} with stderr: {error}", path.display()))?;
            Ok((output, errors))
        });
    let spawned = opened.and_then(|(output, errors)| {
        tokio::process::Command::new("/bin/sh")
            .arg("-c")
            .arg(crate::providers::local::build_job_command(job))
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
        })),
        Err(error) => {
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
        let to = match status {
            Ok(status) if status.success() => {
                job.completed_at = Some(now);
                COMPLETED
            }
            Ok(status) => {
                job.error = Some(format!("the command ended with {status}"));
                job.failed_at = Some(now);
                FAILED
            }
            Err(error) => {
                job.error = Some(format!("the child's exit could not be read: {error}"));
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
