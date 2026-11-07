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
//!
//! The queue's answer about a checkout that carries no marker is kept
//! ([`Answered`]), so it is asked again only about a checkout it has not
//! answered for, and its listing stops once every checkout asked about is
//! proven.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};

use super::{CLEANER, HOME_ROOT};
use crate::providers::local::space_reclaim::janitor::state::report::CleanupReport;
use crate::providers::local::space_reclaim::JanitorError;
use crate::providers::local::work_base::SESSION_CLONE_MARKER;
use crate::targets::ComputeTarget;

/// The record, in the janitor's state directory, of the checkouts below the
/// home this host's queue already answered for.
const ANSWERED_NAME: &str = "session-checkouts-answered.json";

/// What that record is called in its errors.
const ANSWERED_WHAT: &str = "record of the checkouts the queue answered for";

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
/// is returned. The prefixes are listed until every checkout in `asking`
/// is proven ([`by_job`]): more records add proofs and never take one
/// away. `Err` names the read that failed.
pub(super) async fn ask(
    target: &ComputeTarget,
    home: &Path,
    asking: &[&Path],
) -> Result<Vec<SessionJob>, String> {
    let batches = crate::cli::workload::session_batches()
        .map_err(|error| format!("which batch a session is queued under is not known: {error}"))?;
    let store = crate::queue::JobStorage::for_primary_reads()
        .await
        .map_err(|error| {
            format!("the queue store could not be opened for primary reads: {error}")
        })?;
    let mut jobs = Vec::new();
    let prefixes =
        std::iter::once(crate::queue::runs::RUNNING).chain(crate::queue::runs::TERMINAL_PREFIXES);
    for prefix in prefixes {
        let listed = store.list_jobs(prefix).await.map_err(|error| {
            format!("the queue store could not list its {prefix} prefix: {error}")
        })?;
        for job in listed {
            if !batches.contains(&job.batch_id) || !runs_here(target, &job.pinned_host) {
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
        let proven = |checkout: &&Path| matches!(by_job(home, checkout, &jobs), Ok(Some(_)));
        if asking.iter().all(proven) {
            break;
        }
    }
    Ok(jobs)
}

/// The checkouts below the home that carry no clone marker and that this
/// host's queue, listed in full, proved no session cloned: each path with
/// the time its `.git` was made. Such an answer stands, since a session is
/// recorded as started before it clones, so no later record proves a
/// checkout already there; the queue is asked again only about a checkout
/// the record does not hold as it is now (a new one, or one made again),
/// and a host whose person keeps her own checkouts there is not listed job
/// by job on every pass.
pub(super) struct Answered {
    path: PathBuf,
    checkouts: BTreeMap<String, String>,
}

impl Answered {
    /// The record in `state_dir`. One that cannot be read is said on
    /// `report` and holds nothing, so every such checkout is asked about.
    pub(super) fn read(state_dir: &Path, report: &mut CleanupReport) -> Self {
        let path = state_dir.join(ANSWERED_NAME);
        let read = crate::primitives::document::read(&path, ANSWERED_WHAT);
        let checkouts: BTreeMap<String, String> = read
            .unwrap_or_else(|cause| {
                report.add_error(
                    CLEANER,
                    &JanitorError::os(&format!(
                        "{cause}; the queue is asked about every checkout under ~/{HOME_ROOT} \
                         that carries no clone marker, and an enforcing pass writes the record \
                         again"
                    )),
                );
                None
            })
            .unwrap_or_default();
        Self { path, checkouts }
    }

    /// Of `unmarked`, those the record does not hold as they are now.
    pub(super) fn unanswered<'a>(&self, unmarked: &[&'a Path]) -> Vec<&'a Path> {
        unmarked
            .iter()
            .copied()
            .filter(|checkout| {
                let held = self.checkouts.get(&*checkout.to_string_lossy());
                !made_at(checkout).is_ok_and(|made| held == Some(&made.to_rfc3339()))
            })
            .collect()
    }

    /// Write the record again once the queue answered with `jobs`: of
    /// `unmarked`, every checkout no job proves, as it is now; one gone or
    /// proven is dropped. A record that cannot be written is said on
    /// `report`, and the next pass asks again.
    pub(super) fn remember(
        mut self,
        home: &Path,
        unmarked: &[&Path],
        jobs: &[SessionJob],
        report: &mut CleanupReport,
    ) {
        self.checkouts = unmarked
            .iter()
            .filter(|checkout| matches!(by_job(home, checkout, jobs), Ok(None)))
            .filter_map(|checkout| {
                let made = made_at(checkout).ok()?;
                Some((checkout.to_string_lossy().into_owned(), made.to_rfc3339()))
            })
            .collect();
        let written =
            crate::primitives::document::write(&self.path, &self.checkouts, ANSWERED_WHAT);
        if let Err(cause) = written {
            report.add_error(
                CLEANER,
                &JanitorError::os(&format!("{cause}; the next pass asks the queue again")),
            );
        }
    }
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
