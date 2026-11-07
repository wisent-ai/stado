//! What proves that a fleet session cloned one checkout: the only ground on
//! which the janitor takes a checkout. The host's ownership is no proof and
//! neither is the folder a checkout lies in: a person's own clean, fully
//! pushed checkouts sit in the same folders on a host whose registry entry
//! declares no ownership, and they are that person's data.
//!
//! Two proofs, each written by the session that cloned the checkout:
//!
//! - the marker an `omp-session` writes into the checkout's `.git` right
//!   after its clone, holding the job's id
//!   ([`work_base::SESSION_CLONE_MARKER`]);
//! - for a checkout an older release cloned below the home, before the
//!   marker existed: this host's queue record of an `omp-session` it ran
//!   whose command names exactly that checkout, and the checkout's `.git`
//!   made while that job ran (at or after its start, at or before its end).
//!   A session that found a checkout already there worked in it without
//!   cloning it, so a checkout made before the session started is not the
//!   session's, whatever the command names.

use std::path::Path;

use chrono::{DateTime, Utc};

use crate::providers::local::work_base::SESSION_CLONE_MARKER;
use crate::targets::ComputeTarget;

/// The directory a checkout's repository lives in.
const GIT_DIRECTORY: &str = ".git";

/// The word the commands of older releases spelled the account's home with.
const HOME_WORD: &str = "\"$HOME\"/";

/// One `omp-session` this host's queue records as started here.
pub(super) struct SessionJob {
    id: String,
    command: String,
    started: DateTime<Utc>,
    /// When it ended; `None` while it runs.
    ended: Option<DateTime<Utc>>,
}

/// The session that cloned a checkout, as its proof names it.
pub(super) enum Proof {
    /// The clone marker, holding the cloning job's id.
    Marker(String),
    /// The queue record of the job that cloned it.
    Job(String),
}

impl Proof {
    /// The proof in words, for the removal's error and record.
    pub(super) fn describe(&self) -> String {
        match self {
            Self::Marker(job) => format!("its clone marker names job {job}"),
            Self::Job(job) => format!("this host's queue records job {job} cloning it"),
        }
    }
}

/// The clone marker of `checkout` and the job id it holds; `None` when the
/// checkout holds none, which is every checkout no session cloned.
pub(super) fn marker(checkout: &Path) -> Option<Proof> {
    let path = checkout.join(GIT_DIRECTORY).join(SESSION_CLONE_MARKER);
    let info = std::fs::symlink_metadata(&path).ok()?;
    if !info.is_file() {
        return None;
    }
    let job = std::fs::read_to_string(&path).ok()?;
    Some(Proof::Marker(job.trim().to_string()))
}

/// The time an RFC 3339 job stamp names, or `None` when it names none.
fn instant(stamp: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(stamp)
        .ok()
        .map(|parsed| parsed.with_timezone(&Utc))
}

/// Whether a job pinned to `pinned` (`<kind>-<hostname>`, as the submitter
/// writes it) runs on `target`, by the hostnames its registry entry declares.
fn runs_here(target: &ComputeTarget, pinned: &str) -> bool {
    let Some(identity) = pinned.strip_prefix(&format!("{}-", target.kind)) else {
        return false;
    };
    let identity = crate::targets::normalize_hostname(identity);
    target
        .hostnames
        .iter()
        .any(|name| crate::targets::normalize_hostname(name) == identity)
}

/// Every `omp-session` this host's queue records as started on `target`:
/// running, or ended with the time it ended. A job never started cloned
/// nothing, and an ended record stating no end bounds no clone, so neither
/// is returned. `Err` names the read that failed.
pub(super) async fn ask(target: &ComputeTarget) -> Result<Vec<SessionJob>, String> {
    let batch = crate::cli::workload::omp_session_batch();
    let store = crate::queue::JobStorage::for_primary_reads()
        .await
        .map_err(|error| {
            format!("the queue store could not be opened for primary reads: {error}")
        })?;
    let mut jobs = Vec::new();
    let prefixes = std::iter::once(crate::queue::runs::RUNNING)
        .chain(crate::queue::runs::TERMINAL_PREFIXES);
    for prefix in prefixes {
        let listed = store.list_jobs(prefix).await.map_err(|error| {
            format!("the queue store could not list its {prefix} prefix: {error}")
        })?;
        for job in listed {
            if job.batch_id != batch || !runs_here(target, &job.pinned_host) {
                continue;
            }
            let Some(started) = job.started_at.as_deref().and_then(instant) else {
                continue;
            };
            let ended = job
                .completed_at
                .as_deref()
                .or(job.failed_at.as_deref())
                .and_then(instant);
            if ended.is_none() && prefix != crate::queue::runs::RUNNING {
                continue;
            }
            jobs.push(SessionJob {
                id: job.job_id,
                command: job.command,
                started,
                ended,
            });
        }
    }
    Ok(jobs)
}

/// Whether `command` names exactly `token`: the path it spells ends there,
/// so `…/stado` is not named by a command naming `…/stado-landing`.
fn names(command: &str, token: &str) -> bool {
    command.match_indices(token).any(|(at, _)| {
        command[at + token.len()..]
            .chars()
            .next()
            .is_none_or(|next| !(next.is_alphanumeric() || matches!(next, '-' | '_' | '.' | '/')))
    })
}

/// When `checkout`'s repository directory was made.
fn made_at(checkout: &Path) -> Result<DateTime<Utc>, String> {
    let directory = checkout.join(GIT_DIRECTORY);
    std::fs::symlink_metadata(&directory)
        .and_then(|info| info.created())
        .map(DateTime::<Utc>::from)
        .map_err(|error| {
            format!(
                "when {} was made cannot be read ({error}), so whether the session that names \
                 it cloned it is not known",
                directory.display()
            )
        })
}

/// The job among `jobs` that cloned `checkout` below `home`: its command
/// names the checkout as `"$HOME"/<path below the home>`, and the
/// checkout's `.git` was made while it ran. `Ok(None)` when no job proves it;
/// `Err` when a job names it but when the checkout was made cannot be read.
pub(super) fn by_job(
    home: &Path,
    checkout: &Path,
    jobs: &[SessionJob],
) -> Result<Option<Proof>, String> {
    let Ok(relative) = checkout.strip_prefix(home) else {
        return Ok(None);
    };
    let token = format!("{HOME_WORD}{}", relative.display());
    let naming: Vec<&SessionJob> = jobs
        .iter()
        .filter(|job| names(&job.command, &token))
        .collect();
    if naming.is_empty() {
        return Ok(None);
    }
    let made = made_at(checkout)?;
    Ok(naming
        .into_iter()
        .find(|job| job.started <= made && job.ended.is_none_or(|end| made <= end))
        .map(|job| Proof::Job(job.id.clone())))
}
