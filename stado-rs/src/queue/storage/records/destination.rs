//! The destination document a lifecycle transition promises: the current
//! record merged with the fields the requested move is allowed to carry.

use crate::models::Job;
use crate::queue::StorageError;

use super::prefix_state;

pub(in crate::queue::storage) fn merge_transition_destination(
    current: &Job,
    requested: &Job,
    from_prefix: &str,
    to_prefix: &str,
) -> Result<Job, StorageError> {
    if crate::queue::submit::immutable_job_projection(current)
        != crate::queue::submit::immutable_job_projection(requested)
    {
        return Err(StorageError::StorageConflict(format!(
            "{} immutable submission identity changed",
            current.job_id
        )));
    }
    let mut destination = current.clone();
    destination.state = prefix_state(to_prefix).to_string();
    // The worker lease belongs to `running/` and to nothing else: a queued or
    // terminal document carrying an expiry would either fence a reaper that
    // has no worker to lose to, or leave a dead owner's stamp on a finished
    // job.
    destination.lease_expires_at = None;
    match to_prefix {
        "running" => {
            destination.started_at = requested.started_at.clone();
            destination.instance_ref = requested.instance_ref.clone();
            destination.lease_expires_at = requested.lease_expires_at.clone();
        }
        "queue" if from_prefix == "running" => {
            destination.started_at = requested.started_at.clone();
            destination.instance_ref = requested.instance_ref.clone();
            destination.restarts = destination.restarts.max(requested.restarts);
            destination.last_restart = requested.last_restart.clone();
            destination.error = requested.error.clone();
            destination.preempt_count = destination.preempt_count.max(requested.preempt_count);
            destination.yield_count = destination.yield_count.max(requested.yield_count);
            destination.assigned_to = requested.assigned_to.clone();
        }
        "completed" | "uploaded" | "cancelled" => {
            destination.completed_at = requested.completed_at.clone();
            destination.instance_ref = requested.instance_ref.clone();
            destination.error = requested.error.clone();
        }
        "failed" => {
            destination.failed_at = requested.failed_at.clone();
            destination.instance_ref = requested.instance_ref.clone();
            destination.error = requested.error.clone();
        }
        _ => {}
    }
    if crate::queue::runs::TERMINAL_PREFIXES.contains(&to_prefix) {
        destination.peak_vram_gb = destination.peak_vram_gb.max(requested.peak_vram_gb);
        destination.peak_vram_per_gpu |= requested.peak_vram_per_gpu;
        for artifact in &requested.artifact_paths {
            if !destination.artifact_paths.contains(artifact) {
                destination.artifact_paths.push(artifact.clone());
            }
        }
    }
    Ok(destination)
}
