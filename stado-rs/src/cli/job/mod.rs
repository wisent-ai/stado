//! `stado job ...` — per-job rerun, priority and log controls.
//!
//! NO Python original: `stado/cli.py` has no `job` group at all. Items 18
//! and 19 of `stado.wisent.com/docs/missing-commands` are the gap this closes. Re-running
//! a failed job meant retyping its command out of `stado status` and hoping
//! the sizing flags came out the same, and following a running job meant
//! calling `stado machine logs` in a shell loop while tracking the byte
//! cursor by hand.
//!
//! `rerun` never hand-writes a job document. It reads the original,
//! rebuilds the [`SubmitOptions`](crate::queue::submit::SubmitOptions) that produced it and goes back out
//! through [`submit_batch`](crate::queue::submit::submit_batch) — the same entry point `stado submit` uses — so
//! the startup script, the `runs/<run_id>.json` manifest and the
//! `gpu_mem_gb` / `priority` / `gpu_type` blob metadata that
//! [`crate::queue::listing::list_claimable`] prefilters on are all stamped
//! by exactly the code that stamps them for a fresh submit. Resolved
//! hardware is carried explicitly.
//!
//! `watch --follow` carries the byte cursor forward across polls, so every
//! poll prints only the bytes that appeared since the last one and the
//! stream never restarts at zero. One page is the whole remaining log:
//! [`MachineFacade::read_logs`](crate::machine::MachineFacade::read_logs) slices a buffer it has already downloaded,
//! so paging in small windows would cost one extra read per window and save
//! nothing.

use clap::Subcommand;

use crate::machine::MachineError;

use super::CmdError;

mod rerun;
mod set_priority;
mod watch;

use self::rerun::rerun;
use self::set_priority::set_priority;
use self::watch::watch;

#[derive(Subcommand)]
pub enum JobCommands {
    /// Resubmit a job's exact spec under a new job id.
    Rerun {
        /// Job id to copy the spec from, in any lifecycle prefix.
        job_id: String,
        /// Caller-retained token; reuse it after a crash to recover one rerun.
        #[arg(long)]
        retry_token: String,
        /// Emit the original and the rerun as JSON instead of a table.
        #[arg(long)]
        json: bool,
    },
    /// Change a queued job's scheduling priority without resubmitting it.
    SetPriority {
        job_id: String,
        /// New priority; higher jobs are claimed first, then FIFO.
        priority: i64,
        #[arg(long)]
        json: bool,
    },
    /// Print a job's log, and with --follow tail it to a terminal state.
    Watch {
        job_id: String,
        /// Keep polling until the job reaches a terminal prefix.
        #[arg(long)]
        follow: bool,
        /// Buffer the log and emit one JSON object instead of streaming.
        #[arg(long)]
        json: bool,
    },
}

pub async fn dispatch(command: JobCommands) -> Result<(), CmdError> {
    match command {
        JobCommands::Rerun {
            job_id,
            retry_token,
            json,
        } => rerun(&job_id, &retry_token, json).await,
        JobCommands::SetPriority {
            job_id,
            priority,
            json,
        } => set_priority(&job_id, priority, json).await,
        JobCommands::Watch {
            job_id,
            follow,
            json,
        } => watch(&job_id, follow, json).await,
    }
}

/// [`MachineError`] carries a stable code the operator wants next to the
/// message; [`CmdError`] is a flat click exception, so keep both.
fn cmd_error(exc: MachineError) -> CmdError {
    CmdError::click(format!("{}: {}", exc.code, exc.message))
}
