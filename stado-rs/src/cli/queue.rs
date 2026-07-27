//! `stado queue` — maintenance mode: pause, resume, status, drain.
//!
//! NO Python original: the Python CLI has no `queue` group. This is item
//! twenty of `docs/missing-commands.md` ("maintenance mode: stop/start
//! dispatching without cancelling queued jobs") and the operator surface
//! over [`crate::queue::control`] — read that module for what a pause does
//! and, just as importantly, what it deliberately does not do.
//!
//! `drain` is the command every migration runbook already assumed
//! existed: `deploy/migrate_to_stado.sh` refuses to execute unless the
//! operator sets `CONFIRM_FLEET_DRAINED=yes`, and until now nothing could
//! make that claim true.

use std::num::NonZeroUsize;
use std::time::{Duration, Instant};

use clap::Subcommand;
use serde_json::Value;

use crate::constants;
use crate::queue::control::{self, QueueControl};
use crate::queue::JobStorage;

use super::{table, CmdError};

/// Reason stamped by `drain`, which takes no `--reason` of its own — the
/// command IS the reason, and an empty one would make every scheduler and
/// agent log line say nothing.
const DRAIN_REASON: &str = "drained for maintenance (stado queue drain)";

/// `by` sentinel: [`control::set_paused`] resolves an empty operator to
/// this machine's hostname.
const BY_THIS_HOST: &str = "";

#[derive(Subcommand)]
pub enum QueueCommands {
    /// Stop dispatching and claiming; running jobs finish untouched.
    Pause {
        /// Why the queue is paused. Echoed by every scheduler tick and
        /// agent poll that refuses work, so the fleet says why it is idle.
        #[arg(long, default_value = "")]
        reason: String,
    },
    /// Resume dispatching and claiming.
    Resume,
    /// Pause state plus how much work is queued and still running.
    Status {
        #[arg(long)]
        json: bool,
    },
    /// Pause, then (with --wait) block until no job is running any more.
    Drain {
        /// Wait for running/ to empty instead of returning immediately.
        #[arg(long)]
        wait: bool,
        /// Deadline for --wait, in seconds. Exits non-zero if it elapses
        /// with jobs still running.
        #[arg(long, default_value_t = control::default_drain_timeout_s())]
        timeout: u64,
    },
}

pub async fn dispatch(cmd: QueueCommands) -> Result<(), CmdError> {
    match cmd {
        QueueCommands::Pause { reason } => pause(&reason).await,
        QueueCommands::Resume => resume().await,
        QueueCommands::Status { json } => status(json).await,
        QueueCommands::Drain { wait, timeout } => drain(wait, timeout).await,
    }
}

/// Python `click.echo(json.dumps(payload, indent=2, sort_keys=True))`, the
/// same shape `cli/quota.rs::echo_json` prints.
fn echo_json(value: &Value) {
    let pretty = serde_json::to_string_pretty(value).expect("Value serialization is infallible");
    println!("{}", crate::models::ensure_ascii(&pretty));
}

/// The switch as FIELD/VALUE rows for [`table::print`].
fn state_rows(state: &QueueControl) -> Vec<Vec<String>> {
    vec![
        vec!["paused".to_string(), state.paused.to_string()],
        vec!["reason".to_string(), state.reason.clone()],
        vec!["since".to_string(), state.since.clone()],
        vec!["by".to_string(), state.by.clone()],
    ]
}

fn print_state(state: &QueueControl) {
    table::print(&["FIELD", "VALUE"], &state_rows(state));
}

async fn pause(reason: &str) -> Result<(), CmdError> {
    let store = JobStorage::new().await?;
    let state = control::set_paused(&store, true, reason, BY_THIS_HOST).await?;
    print_state(&state);
    println!(
        "\nDispatch and new claims are stopped. Jobs already running keep going and \
         queued jobs are untouched — resume with `stado queue resume`, or wait them out \
         with `stado queue drain --wait`."
    );
    Ok(())
}

async fn resume() -> Result<(), CmdError> {
    let store = JobStorage::new().await?;
    // Clearing the reason is deliberate: a resumed queue carrying the old
    // "why we paused" note would misreport the fleet forever.
    let state = control::set_paused(&store, false, "", BY_THIS_HOST).await?;
    print_state(&state);
    println!(
        "\nDispatching resumes on the next coordinator tick; agents claim on their next poll."
    );
    Ok(())
}

async fn status(as_json: bool) -> Result<(), CmdError> {
    let store = JobStorage::new().await?;
    let state = control::read(&store).await?;
    let mut counts: Vec<(&str, usize)> = Vec::with_capacity(control::WATCHED_PREFIXES.len());
    for prefix in control::WATCHED_PREFIXES {
        counts.push((*prefix, control::job_count(&store, prefix).await?));
    }

    if as_json {
        let mut payload = serde_json::to_value(&state)?;
        let per_prefix: serde_json::Map<String, Value> = counts
            .iter()
            .map(|(prefix, count)| ((*prefix).to_string(), Value::from(*count)))
            .collect();
        payload["counts"] = Value::Object(per_prefix);
        echo_json(&payload);
        return Ok(());
    }

    print_state(&state);
    let rows: Vec<Vec<String>> = counts
        .iter()
        .map(|(prefix, count)| vec![(*prefix).to_string(), count.to_string()])
        .collect();
    table::print(&["PREFIX", "JOBS"], &rows);
    Ok(())
}

async fn drain(wait: bool, timeout_s: u64) -> Result<(), CmdError> {
    let store = JobStorage::new().await?;
    let state = control::set_paused(&store, true, DRAIN_REASON, BY_THIS_HOST).await?;
    print_state(&state);

    if !wait {
        println!(
            "\nPaused. Nothing new dispatches or gets claimed, but jobs already in running/ \
             are still going — the fleet is NOT drained yet. Re-run with --wait, or watch \
             `stado queue status`, before copying storage."
        );
        return Ok(());
    }

    // Poll at the agent's own cadence: a slot leaves running/ when its
    // agent notices the child exited, which happens once per
    // `constants::POLL_INTERVAL_S`. Listing faster only adds storage
    // traffic to a fleet that is trying to go quiet.
    let poll = Duration::from_secs(constants::POLL_INTERVAL_S);
    let budget = Duration::from_secs(timeout_s);
    let started = Instant::now();
    println!();
    loop {
        let running = control::job_count(&store, control::RUNNING_PREFIX).await?;
        let elapsed = started.elapsed();
        let Some(remaining) = NonZeroUsize::new(running) else {
            let queued = control::job_count(&store, control::QUEUED_PREFIX).await?;
            println!(
                "drained after {}s: running/ is empty. {queued} job(s) remain queued and \
                 untouched — pausing is not cancelling, and they dispatch again on \
                 `stado queue resume`.",
                elapsed.as_secs()
            );
            return Ok(());
        };
        if elapsed >= budget {
            return Err(CmdError::click(format!(
                "drain timed out after {}s with {remaining} job(s) still in running/. The \
                 queue stays PAUSED, so nothing new was dispatched or claimed: re-run \
                 `stado queue drain --wait` to keep waiting, cancel the stragglers with \
                 `stado cancel`, or `stado queue resume` to abandon the drain. Do NOT \
                 copy storage until running/ is empty.",
                elapsed.as_secs()
            )));
        }
        println!(
            "waiting: {remaining} job(s) still running ({}s elapsed)",
            elapsed.as_secs()
        );
        tokio::time::sleep(poll).await;
    }
}
