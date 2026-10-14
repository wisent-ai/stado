//! The exit status a finished tail hands back to the shell.

use crate::cli::CmdError;
use crate::models::{job_state, Job};

/// The exit status carries the job's outcome, so
/// `stado job watch ID --follow && next-step` means what it reads
/// like. A job that is still running is not a failure — without
/// `--follow` the operator asked for a snapshot, not a verdict. In
/// `--json` mode the message goes to stderr and stdout stays a single
/// parseable object.
pub(super) fn outcome(job: &Job, terminal: bool) -> Result<(), CmdError> {
    if !terminal {
        return Ok(());
    }
    match job.state.as_str() {
        job_state::FAILED | job_state::CANCELLED => {
            // A job that ended without recording why says so, rather than
            // ending the sentence where the cause would be.
            let detail = match job.error.as_deref() {
                Some(err) => format!(": {err}"),
                None => ", and its record names no error".to_string(),
            };
            Err(CmdError::unreachable(format!(
                "job {} ended {}{detail}",
                job.job_id, job.state
            )))
        }
        _ => Ok(()),
    }
}
