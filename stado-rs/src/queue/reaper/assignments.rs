//! The second pass: queued jobs pinned to a worker that stopped
//! broadcasting capacity, released back to any eligible claimant.

use chrono::Utc;

use crate::models::{job_state, Job};
use crate::queue::{capacity, JobStorage, StorageError};

use super::ReaperSummary;

/// Clear `assigned_to` on queued jobs whose named worker has gone silent,
/// so another worker can claim them. Silence is the codebase's own
/// liveness horizon: absence from [`capacity::read_consumer_capacity`],
/// which drops every publication older than
/// [`capacity::CAPACITY_STALE_SECONDS`]. Operator hard-pins
/// (`pinned_host`) are never touched — the same rule the makespan matcher
/// follows.
pub(super) async fn clear_silent_assignments(
    store: &JobStorage,
    now: chrono::DateTime<Utc>,
    log: &dyn Fn(&str),
    summary: &mut ReaperSummary,
) -> Result<(), StorageError> {
    let live = capacity::read_consumer_capacity(store).await?;
    for candidate in store.list_jobs("queue", 0).await? {
        store.recover_job_transition(&candidate.job_id).await?;
        let Some(current) = store.read_job("queue", &candidate.job_id).await? else {
            continue;
        };
        if current.state != job_state::QUEUED {
            continue;
        }
        if current.assigned_to.is_empty() || !current.pinned_host.is_empty() {
            continue;
        }
        let worker_live = live
            .keys()
            .any(|consumer| consumer.eq_ignore_ascii_case(&current.assigned_to));
        if worker_live {
            continue;
        }
        let job_id = current.job_id.clone();
        let path = format!("queue/{job_id}.json");
        let Some(versioned) = store.read_text_versioned(&path).await? else {
            continue;
        };
        let mut job = Job::from_json(&versioned.content)?;
        if job.state != job_state::QUEUED || job.assigned_to != current.assigned_to {
            continue; // changed under the listing; the next tick reconciles
        }
        let worker = std::mem::take(&mut job.assigned_to);
        match store
            .compare_and_swap_text(&path, &versioned.version, &job.to_json())
            .await
        {
            Ok(_) => {
                summary.assignments_cleared += 1;
                let broadcast = store
                    .backend()
                    .updated_at(&format!("{}{worker}.json", capacity::CAPACITY_PREFIX))
                    .await?;
                let silence = match broadcast {
                    Some(updated) => {
                        format!("last broadcast {}s ago", (now - updated).num_seconds())
                    }
                    None => "no capacity broadcast on record".to_string(),
                };
                log(&format!(
                    "{job_id}: cleared assignment to silent worker {worker} ({silence})"
                ));
            }
            Err(StorageError::StorageConflict(_)) => continue,
            Err(error) => return Err(error),
        }
    }
    Ok(())
}
