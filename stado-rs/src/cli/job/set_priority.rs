//! `stado job set-priority` — reorder a job that has not been claimed yet.

use serde_json::json;

use crate::cli::{reporting::table, CmdError};
use crate::machine::MachineFacade;
use crate::queue::JobStorage;

use super::cmd_error;

/// `stado job set-priority JOB_ID PRIORITY` — atomically update the queued
/// document, its scheduler metadata and its priority marker.
pub(super) async fn set_priority(
    job_id: &str,
    priority: i64,
    as_json: bool,
) -> Result<(), CmdError> {
    let store = JobStorage::new().await?;
    let Some(job) = store.update_queued_priority(job_id, priority).await? else {
        let current = MachineFacade::new()
            .await
            .map_err(cmd_error)?
            .lookup_job(job_id)
            .await
            .map_err(cmd_error)?;
        return Err(CmdError::click(format!(
            "job {job_id} is {}, not queued; its priority was not changed",
            current.state
        )));
    };

    if as_json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "job_id": job.job_id,
                "state": job.state,
                "priority": job.priority,
            }))?
        );
    } else {
        table::print(
            &["JOB", "STATE", "PRIORITY"],
            &[vec![job.job_id, job.state, job.priority.to_string()]],
        );
    }
    Ok(())
}
