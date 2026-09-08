//! The exit status a finished tail hands back to the shell.

use crate::cli::CmdError;
use crate::models::{job_state, Job};

/// The exit status carries the job's outcome, so
/// `stado job watch ID --follow && next-step` means what it reads like. A
/// job that is still running is not a failure — without `--follow` the
/// operator asked for a snapshot, not a verdict. In `--json` mode the
/// message goes to stderr and stdout stays a single parseable object.
pub(super) fn outcome(job: &Job, terminal: bool) -> Result<(), CmdError> {
    if !terminal {
        return Ok(());
    }
    match job.state.as_str() {
        job_state::FAILED | job_state::CANCELLED => {
            let detail = job
                .error
                .as_deref()
                .map(|err| format!(": {err}"))
                .unwrap_or_default();
            Err(CmdError::click(format!(
                "job {} ended {}{detail}",
                job.job_id, job.state
            )))
        }
        _ => Ok(()),
    }
}
