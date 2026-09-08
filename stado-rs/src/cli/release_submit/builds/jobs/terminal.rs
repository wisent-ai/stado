//! Waiting for one release job to reach a terminal queue state, and the
//! evidence its own log carries when it failed.

use std::time::Duration;

use crate::cli::cancel;
use crate::cli::CmdError;
use crate::models::Job;
use crate::queue::storage::JobStorage;

pub(crate) async fn read_terminal_job(
    store: &JobStorage,
    id: &str,
) -> Result<Option<Job>, CmdError> {
    for prefix in crate::queue::runs::TERMINAL_PREFIXES {
        if let Some(job) = store.read_job(prefix, id).await? {
            return Ok(Some(job));
        }
    }
    Ok(None)
}

pub(crate) async fn terminal(store: &JobStorage, id: &str) -> Result<Job, CmdError> {
    loop {
        if let Some(job) = read_terminal_job(store, id).await? {
            return Ok(job);
        }
        if store.read_job("queue", id).await?.is_some() {
            let queue_control = crate::queue::control::read(store)
                .await
                .map_err(|error| CmdError::click(error.to_string()))?;
            if queue_control.paused {
                // A product release runs on the same publisher runner as a
                // Stado release. Holding that runner while maintenance keeps
                // this job queued prevents the release that can resume the
                // fleet from ever starting.
                cancel::cancel_in_store(store, id).await?;
                return Err(CmdError::click(format!(
                    "cancelled queued release job {id} because the queue is paused ({})",
                    queue_control.pause_summary()
                )));
            }
        }
        tokio::time::sleep(Duration::from_secs(3)).await
    }
}

/// The last lines the failed job wrote, so the failure carries its own
/// evidence.
///
/// A failed release job used to surface only the queue's one-size verdict
/// ("workload exited unsuccessfully; inspect the redacted command output"),
/// which sent the operator hunting per host. The worker names its steps in
/// that log, so its tail is the diagnosis; it travels in the CLI error and,
/// through the platform failure field, into the persisted run object the
/// dashboard serves.
pub(crate) async fn job_output_tail(store: &JobStorage, job_id: &str) -> String {
    let path = format!("status/{job_id}/output/command_output.log");
    match store.read_bytes(&path).await {
        Ok(Some(bytes)) => {
            let text = String::from_utf8_lossy(&bytes);
            let lines: Vec<&str> = text.lines().collect();
            let tail = &lines[lines.len().saturating_sub(15)..];
            format!("; the job's last output:\n{}", tail.join("\n"))
        }
        Ok(None) => "; the job left no output log".to_string(),
        Err(error) => format!("; the job's output log could not be read: {error}"),
    }
}
