//! Retire the cleaned sentinels a finished job leaves behind in the prefix
//! it left.
//!
//! Every durable transition fences its source document, writes the
//! destination, then rewrites the source as a `transition-cleaned:<id>`
//! sentinel and never deletes it (`transitions/retire.rs`). The sentinel is
//! what lets an interrupted transition be finished by anyone and what tells
//! the workdir cleaner the source is settled — and it is also why `queue/`
//! held 779 objects for 15 queued jobs on 2026-09-17. Every reader that
//! wants the queued jobs downloads all of them: the scheduler's window, the
//! janitor's keep-list, `stado status`, and `stado host gates`, which timed
//! out at ten seconds on every host over 764 sentinels nobody would ever
//! read again.
//!
//! A sentinel is retired here only when nothing can want it: the job's
//! transition record is retired (or gone), and its destination is a terminal
//! prefix. A terminal job id never re-enters `queue/` or `running/` — a rerun
//! is a new id — so no writer races the delete, which is why an
//! unconditional delete is safe for exactly this set and for no other.

use chrono::{DateTime, Duration, Utc};

use crate::models::Job;
use crate::queue::runs::TERMINAL_PREFIXES;
use crate::queue::storage::records::{cleaned_transition_id, transition_path, JobTransition};
use crate::queue::storage::{transition_is_retired, JobStorage};
use crate::queue::StorageError;

/// How old a source object must be before the sweep reads it at all. A
/// sentinel younger than this may belong to a transition still finishing;
/// the workdir cleaner and recovery read it in that window.
pub const SETTLED_SENTINEL_MIN_AGE: Duration = Duration::hours(24);

/// What one bounded sweep did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SettledSentinelSweep {
    /// Objects older than the age floor the sweep downloaded.
    pub inspected: usize,
    /// Sentinels deleted because their job is terminal and settled.
    pub retired: usize,
    /// Old objects that were live jobs or sentinels of jobs not yet
    /// terminal; left in place.
    pub kept: usize,
    /// Whether the per-call budget stopped the sweep before the prefix was
    /// exhausted.
    pub budget_exhausted: bool,
}

impl JobStorage {
    /// One bounded pass over `{prefix}/`: delete every cleaned sentinel whose
    /// job has settled in a terminal prefix, oldest objects first, reading
    /// at most `budget` bodies.
    pub async fn retire_settled_sentinels(
        &self,
        prefix: &str,
        now: DateTime<Utc>,
        budget: usize,
    ) -> Result<SettledSentinelSweep, StorageError> {
        let directory = format!("{prefix}/");
        let floor = now - SETTLED_SENTINEL_MIN_AGE;
        let mut old: Vec<_> = self
            .list_blobs_with_meta(&directory)
            .await?
            .into_iter()
            .filter(|blob| {
                blob.name.starts_with(&directory)
                    && blob.name.ends_with(".json")
                    && blob.updated.is_some_and(|updated| updated <= floor)
            })
            .collect();
        old.sort_by_key(|blob| blob.updated);
        let mut sweep = SettledSentinelSweep::default();
        for blob in old {
            if sweep.inspected >= budget {
                sweep.budget_exhausted = true;
                break;
            }
            sweep.inspected += 1;
            let Some(body) = self.download_text(&blob.name).await? else {
                continue;
            };
            let Ok(job) = Job::from_json(&body) else {
                sweep.kept += 1;
                continue;
            };
            if cleaned_transition_id(&job.state).is_none() {
                sweep.kept += 1;
                continue;
            }
            if self.job_is_settled_terminal(&job.job_id).await? {
                self.delete_blob(&blob.name).await?;
                sweep.retired += 1;
            } else {
                sweep.kept += 1;
            }
        }
        Ok(sweep)
    }

    /// Whether the job's latest transition is retired and landed in a prefix
    /// it can never leave. A missing record counts as settled only when the
    /// job is found in a terminal prefix, because the record is the only
    /// other witness to where it went.
    async fn job_is_settled_terminal(&self, job_id: &str) -> Result<bool, StorageError> {
        match self.download_text(&transition_path(job_id)).await? {
            Some(record) => {
                let transition: JobTransition = serde_json::from_str(&record)?;
                Ok(transition_is_retired(&transition.state)
                    && TERMINAL_PREFIXES.contains(&transition.to_prefix.as_str()))
            }
            None => {
                for prefix in TERMINAL_PREFIXES {
                    if self.read_job(prefix, job_id).await?.is_some() {
                        return Ok(true);
                    }
                }
                Ok(false)
            }
        }
    }
}
