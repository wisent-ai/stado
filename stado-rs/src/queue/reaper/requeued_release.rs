//! The third pass: a release job the reaper requeued on an expired lease
//! whose first worker kept building and published a passed receipt after the
//! requeue. Its result exists, so running it again spends a builder on a
//! build that is already done.
//!
//! The job is completed from its queue record by the same evidence the
//! running-job pass reads ([`verified_release_completion`]): the receipt and
//! archive must verify against the job's immutable request and be written at
//! or after the job was created. A job some worker has claimed again is in
//! `running/`, not `queue/`, and is left to the running-job pass.

use chrono::Utc;

use crate::models::{job_state, Job};
use crate::monitor::heartbeat_guard as hg;
use crate::queue::{JobStorage, StorageError};

use super::release_output::verified_release_completion;
use super::{ReaperSummary, LEASE_EXPIRED_REASON};

/// Complete every queued release job requeued on an expired lease whose
/// requeued execution published a verified result.
pub(super) async fn complete_requeued_releases(
    store: &JobStorage,
    now: chrono::DateTime<Utc>,
    log: &dyn Fn(&str),
    summary: &mut ReaperSummary,
) -> Result<(), StorageError> {
    let mut completed = Vec::new();
    for job_id in store.list_job_ids("queue").await? {
        let path = format!("queue/{job_id}.json");
        let Some(versioned) = store.read_text_versioned(&path).await? else {
            continue;
        };
        let mut job = Job::from_json(&versioned.content)?;
        if job.state != job_state::QUEUED || job.error.as_deref() != Some(LEASE_EXPIRED_REASON) {
            continue;
        }
        let Some(created) = hg::parse_iso_lenient(&job.created_at) else {
            log(&format!(
                "{job_id}: requeued job has no readable created_at, so its retained release \
                 output cannot be tied to it"
            ));
            continue;
        };
        let Some(completed_at) =
            verified_release_completion(store, &job, created, now, log).await?
        else {
            continue;
        };
        job.state = job_state::COMPLETED.to_string();
        job.completed_at = Some(completed_at);
        job.failed_at = None;
        job.error = None;
        if store
            .move_job_if_version(&job, "queue", "completed", &versioned.version)
            .await?
        {
            log(&format!(
                "{job_id}: requeued on an expired lease, completed from the verified release \
                 output its first worker published after the requeue"
            ));
            completed.push(job_id);
        }
    }
    summary.release_completions += completed.len();
    Ok(())
}
