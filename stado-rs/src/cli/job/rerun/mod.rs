//! `stado job rerun` — read a job, rebuild its submit options, resubmit.

use serde_json::json;

use crate::cli::{reporting::table, CmdError};
use crate::machine::{normalize_job, MachineFacade};
use crate::queue::submit::submit_batch;

use super::cmd_error;

mod options;
mod spec_rows;

use self::options::rerun_options;
use self::spec_rows::spec_rows;

/// `stado job rerun JOB_ID --retry-token TOKEN [--json]` — resubmit an
/// identical spec under a deterministic durable run and print `old -> new`.
pub(super) async fn rerun(job_id: &str, retry_token: &str, json: bool) -> Result<(), CmdError> {
    if retry_token.trim().is_empty() {
        return Err(CmdError::click("--retry-token must not be empty"));
    }
    let facade = MachineFacade::new().await.map_err(cmd_error)?;
    // lookup_job probes machine::JOB_PREFIXES, which is the same six
    // prefixes as queue::runs::ALL_PREFIXES — including `cancelled/`, the
    // one JobStorage::list_all_jobs still omits. A cancelled job is exactly
    // the kind an operator reruns, so the listing helper is the wrong seam
    // here.
    let original = facade.lookup_job(job_id).await.map_err(cmd_error)?;

    let options = rerun_options(&original, retry_token);
    let submitted = submit_batch(std::slice::from_ref(&original.command), &options).await?;
    let Some(fresh) = submitted.into_iter().next() else {
        return Err(CmdError::click(format!(
            "resubmitting {job_id} returned no job; nothing was queued"
        )));
    };

    if json {
        let payload = json!({
            "original": normalize_job(&original),
            "rerun": normalize_job(&fresh),
        });
        println!("{}", serde_json::to_string_pretty(&payload)?);
        return Ok(());
    }

    println!("{} -> {}", original.job_id, fresh.job_id);
    println!("{}", fresh.command);
    table::print(
        &["FIELD", "ORIGINAL", "RERUN"],
        &spec_rows(&original, &fresh),
    );
    Ok(())
}
