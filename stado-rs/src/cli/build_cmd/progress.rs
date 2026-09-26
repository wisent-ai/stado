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
    pub seconds: i64,
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
    /// When the host last wrote the job's log, and how long ago. The agent
    /// rewrites it on every heartbeat, so a job whose log stopped moving is a
    /// job no host is running any more, whatever the queue still says: on
    /// 2026-09-26 three superseded Stado build jobs read `running for 7h`
    /// while their logs had not changed since the evening before.
    pub last_output: Option<LastOutput>,
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
            if let Ok(at) = DateTime::parse_from_rfc3339(at) {
                let at = at.with_timezone(&Utc);
                progress.running = Some(Running {
                    name: name.to_owned(),
                    since: at.to_rfc3339(),
                    seconds: (now - at).num_seconds().max(0),
                    blocked_on: None,
                });
            }
        } else if let Some((name, exit)) = rest.split_once(EXIT) {
            let (exit, seconds) = match exit.rsplit_once(AFTER) {
                Some((exit, seconds)) => (exit, seconds.trim_end_matches('s').parse().ok()),
                None => (exit, None),
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
                exit: exit.to_owned(),
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
            progress.last_output = store
                .updated_at(&format!("status/{job_id}/output/command_output.log"))
                .await
                .ok()
                .flatten()
                .map(|at| LastOutput {
                    at: at.to_rfc3339(),
                    seconds_ago: (now - at).num_seconds().max(0),
                });
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
            Some(running) => format!(
                "step {}: running for {}s (since {}){}",
                running.name,
                running.seconds,
                running.since,
                running
                    .blocked_on
                    .as_deref()
                    .map(|blocked| format!("; blocked: {blocked}"))
                    .unwrap_or_default()
            ),
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
        lines
    }
}
