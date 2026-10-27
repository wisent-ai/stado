//! What a platform job is doing and what its steps cost, read back from the
//! queue and from the job's own streamed log.
//!
//! `stado build status` used to answer `state=building` for the whole of a
//! 35-45 minute build: nothing said whether the job still sat in the queue,
//! which step it was in, since when, or what the finished steps had cost.
//! The worker writes `step <name>: started at <time>` and
//! `step <name>: exit <code> after <n>s` (see `worker/steps.rs`), the host
//! agent streams that log to `status/<job>/output/command_output.log` on
//! every heartbeat, and this module turns it into those answers for the
//! text report and for `--json`, which is what Stado Desktop shows.

use chrono::{DateTime, Utc};
use serde::Serialize;

use crate::queue::storage::JobStorage;

const STEP_PREFIX: &str = "[release-worker] step ";
const STARTED: &str = ": started at ";
const EXIT: &str = ": exit ";
const AFTER: &str = " after ";

/// One step the worker finished.
#[derive(Debug, PartialEq, Serialize)]
pub(crate) struct Finished {
    pub name: String,
    pub exit: String,
    /// Absent for a log written before steps carried their duration.
    pub seconds: Option<u64>,
}

/// The step the worker is in now.
#[derive(Debug, PartialEq, Serialize)]
pub(crate) struct Running {
    pub name: String,
    pub since: String,
    /// Seconds since the step started; absent when its start time could not
    /// be read, negative when the host's clock runs ahead of this one.
    pub seconds: Option<i64>,
    /// What the step is stuck behind, when its output says so.
    pub blocked_on: Option<String>,
}

/// What Cargo prints while another Cargo process on the same host holds the
/// build directory or package cache this one needs.
const CARGO_LOCK_WAIT: &str = "Blocking waiting for file lock";

/// One platform job as far as the queue and its log tell.
#[derive(Debug, Default, PartialEq, Serialize)]
pub(crate) struct Progress {
    /// What the job waits for before any step can run, or why its log could
    /// not be read.
    pub waiting: Option<String>,
    pub steps: Vec<Finished>,
    pub running: Option<Running>,
    /// Last host write and its age, independently of the queue's job state.
    /// The agent refreshes the log on heartbeats, exposing stale progress
    /// even when the queue still reports the job as running.
    pub last_output: Option<LastOutput>,
    /// Why the log's last write time could not be read; without it a stale
    /// job lost the one line that shows it is stale, with no word why.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_output_error: Option<String>,
}

/// The last write of a job's streamed log.
#[derive(Debug, PartialEq, Serialize)]
pub(crate) struct LastOutput {
    pub at: String,
    pub seconds_ago: i64,
}

/// Read the worker's step lines out of one job log.
pub(crate) fn steps(log: &str, now: DateTime<Utc>) -> Progress {
    let mut progress = Progress::default();
    for line in log.lines() {
        let Some(rest) = line.trim_end().strip_prefix(STEP_PREFIX) else {
            continue;
        };
        if let Some((name, at)) = rest.split_once(STARTED) {
            // A start time that does not parse is still a running step; the
            // report says the time could not be read rather than dropping it.
            // A negative elapsed time is the host's clock running ahead of
            // this one, and is shown as measured.
            progress.running = Some(match DateTime::parse_from_rfc3339(at) {
                Ok(at) => {
                    let at = at.with_timezone(&Utc);
                    Running {
                        name: name.to_owned(),
                        since: at.to_rfc3339(),
                        seconds: Some((now - at).num_seconds()),
                        blocked_on: None,
                    }
                }
                Err(error) => Running {
                    name: name.to_owned(),
                    since: format!("{at:?}, which is not RFC 3339 ({error})"),
                    seconds: None,
                    blocked_on: None,
                },
            });
        } else if let Some((name, exit)) = rest.split_once(EXIT) {
            let (exit, seconds) = match exit.rsplit_once(AFTER) {
                Some((exit, raw)) => match raw.trim_end_matches('s').parse::<u64>() {
                    Ok(seconds) => (exit.to_owned(), Some(seconds)),
                    Err(error) => (format!("{exit} (duration {raw:?} unreadable: {error})"), None),
                },
                None => (exit.to_owned(), None),
            };
            if progress
                .running
                .as_ref()
                .is_some_and(|running| running.name == name)
            {
                progress.running = None;
            }
            progress.steps.push(Finished {
                name: name.to_owned(),
                exit,
                seconds,
            });
        }
    }
    // A step whose newest line is Cargo's lock wait is not compiling: another
    // build on this host holds the shared build directory. A Stado darwin job
    // spent 49 minutes on that one line, and `running for` alone read as a
    // slow compile.
    if let Some(running) = progress.running.as_mut() {
        let last = log.lines().rev().find(|line| !line.trim().is_empty());
        if let Some(line) = last.filter(|line| line.trim_start().starts_with(CARGO_LOCK_WAIT)) {
            running.blocked_on = Some(format!(
                "{} — another Cargo process on this host holds it",
                line.trim()
            ));
        }
    }
    progress
}

/// Where one platform job stands: still queued, not yet writing, or the
/// steps its log names.
pub(crate) async fn read(store: &JobStorage, job_id: &str, now: DateTime<Utc>) -> Progress {
    let waiting = |detail: String| Progress {
        waiting: Some(detail),
        ..Progress::default()
    };
    match store.read_job("queue", job_id).await {
        Ok(Some(job)) => {
            let host = if job.pinned_host.is_empty() {
                "any eligible host".to_owned()
            } else {
                job.pinned_host.clone()
            };
            return waiting(format!(
                "queued since {} for {host} to claim it (queue state {})",
                job.created_at, job.state
            ));
        }
        Ok(None) => {}
        Err(error) => return waiting(format!("the queue could not be read: {error}")),
    }
    match store
        .read_bytes(&format!("status/{job_id}/output/command_output.log"))
        .await
    {
        Ok(Some(bytes)) => {
            let mut progress = steps(&String::from_utf8_lossy(&bytes), now);
            let path = format!("status/{job_id}/output/command_output.log");
            match store.updated_at(&path).await {
                Ok(at) => {
                    progress.last_output = at.map(|at| LastOutput {
                        at: at.to_rfc3339(),
                        seconds_ago: (now - at).num_seconds(),
                    })
                }
                Err(error) => progress.last_output_error = Some(error.to_string()),
            }
            progress
        }
        Ok(None) => waiting("claimed; its host has streamed no output yet".to_owned()),
        Err(error) => waiting(format!("its output log could not be read: {error}")),
    }
}

impl Progress {
    /// The report's lines; `building` adds what a live job is doing now.
    pub(crate) fn lines(&self, building: bool) -> Vec<String> {
        let mut lines: Vec<String> = self.waiting.iter().cloned().collect();
        lines.extend(self.steps.iter().map(|step| match step.seconds {
            Some(seconds) => format!("step {}: exit {} after {seconds}s", step.name, step.exit),
            None => format!("step {}: exit {}", step.name, step.exit),
        }));
        if !building || self.waiting.is_some() {
            return lines;
        }
        lines.push(match &self.running {
            Some(running) => {
                let elapsed = match running.seconds {
                    Some(seconds) => format!("running for {seconds}s"),
                    None => "running".to_owned(),
                };
                let blocked = match running.blocked_on.as_deref() {
                    Some(blocked) => format!("; blocked: {blocked}"),
                    None => String::new(),
                };
                format!("step {}: {elapsed} (since {}){blocked}", running.name, running.since)
            }
            None if self.steps.is_empty() => {
                "preparing the source and toolchain; no step has started".to_owned()
            }
            None => "between steps: the last one ended and the next has not started".to_owned(),
        });
        if let Some(last) = &self.last_output {
            lines.push(format!(
                "host last streamed output {}s ago (at {})",
                last.seconds_ago, last.at
            ));
        }
        if let Some(error) = &self.last_output_error {
            lines.push(format!(
                "when the host last streamed output could not be read: {error}"
            ));
        }
        lines
    }
}
