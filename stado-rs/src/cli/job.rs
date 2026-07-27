//! `stado job ...` — the per-job operator surface: rerun and watch.
//!
//! NO Python original: `stado/cli.py` has no `job` group at all. Items 18
//! and 19 of `docs/missing-commands.md` are the gap this closes. Re-running
//! a failed job meant retyping its command out of `stado status` and hoping
//! the sizing flags came out the same, and following a running job meant
//! calling `stado machine logs` in a shell loop while tracking the byte
//! cursor by hand.
//!
//! `rerun` never hand-writes a job document. It reads the original,
//! rebuilds the [`SubmitOptions`] that produced it and goes back out
//! through [`submit_batch`] — the same entry point `stado submit` uses — so
//! the startup script, the `runs/<run_id>.json` manifest and the
//! `gpu_mem_gb` / `priority` / `gpu_type` blob metadata that
//! [`crate::queue::listing::list_fitting`] prefilters on are all stamped by
//! exactly the code that stamps them for a fresh submit. `rerun_options`
//! documents the one place the resolved spec cannot simply be pinned back.
//!
//! `watch --follow` carries the byte cursor forward across polls, so every
//! poll prints only the bytes that appeared since the last one and the
//! stream never restarts at zero. One page is the whole remaining log:
//! [`MachineFacade::read_logs`] slices a buffer it has already downloaded,
//! so paging in small windows would cost one extra read per window and save
//! nothing.

use std::io::Write;
use std::time::Duration;

use clap::Subcommand;
use serde_json::json;

use crate::constants::POLL_INTERVAL_S;
use crate::machine::{normalize_job, MachineError, MachineFacade};
use crate::models::{job_state, Job};
use crate::queue::submit::{submit_batch, SubmitOptions, CPU_MACHINE_TYPE};

use super::{table, CmdError};

/// Poll cadence for `--follow`. The bytes being tailed are written by an
/// agent that polls the same store at [`POLL_INTERVAL_S`]; reading faster
/// than the writer writes only multiplies storage round-trips, so the tail
/// rides the fleet's own cadence.
const WATCH_POLL_INTERVAL: Duration = Duration::from_secs(POLL_INTERVAL_S);

/// Bytes requested per log page. [`MachineFacade::read_logs`] slices an
/// already-downloaded buffer, so the cheapest page is "all of it" — one
/// read per poll instead of one per window. `u32::MAX` is a digit-free
/// bound far above any command log, and [`drain`] still honours `eof` if
/// one ever exceeds it.
const LOG_PAGE_BYTES: i64 = u32::MAX as i64;

#[derive(Subcommand)]
pub enum JobCommands {
    /// Resubmit a job's exact spec under a new job id.
    Rerun {
        /// Job id to copy the spec from, in any lifecycle prefix.
        job_id: String,
        /// Emit the original and the rerun as JSON instead of a table.
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
        JobCommands::Rerun { job_id, json } => rerun(&job_id, json).await,
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

/// `stado job rerun JOB_ID [--json]` — resubmit an identical spec as a new
/// job id and print `old -> new`.
async fn rerun(job_id: &str, json: bool) -> Result<(), CmdError> {
    let facade = MachineFacade::new().await.map_err(cmd_error)?;
    // lookup_job probes machine::JOB_PREFIXES, which is the same six
    // prefixes as queue::runs::ALL_PREFIXES — including `cancelled/`, the
    // one JobStorage::list_all_jobs still omits. A cancelled job is exactly
    // the kind an operator reruns, so the listing helper is the wrong seam
    // here.
    let original = facade.lookup_job(job_id).await.map_err(cmd_error)?;

    let options = rerun_options(&original);
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

/// The [`SubmitOptions`] that reproduce `original`'s spec.
///
/// Every routing field is pinned to what the original RESOLVED to rather
/// than to the flags that produced it — the job document records the former
/// and never the latter — so the rerun lands on the same hardware. That is
/// the whole content of "resubmit with identical spec".
///
/// The one exception is a job that came out of the CPU branch of
/// `queue::submit::submit_via_gcs`, recognizable by [`CPU_MACHINE_TYPE`]
/// with no accelerator and no sized VRAM. That branch runs only when the
/// caller pinned nothing, so pinning its machine_type back would flip
/// submit's `caller_asked_for_gpu` gate and stamp an accelerator onto a CPU
/// job. Leaving all three empty re-enters the same branch: a command that
/// still sizes to nothing comes out byte-identical, and one the fleet has
/// since measured gets today's real size — which is precisely what
/// resubmitting the same spec should do, since the original's zero recorded
/// "not sized yet", not "needs no GPU".
///
/// `re_submission_of` is set to the id being rerun. That is what makes
/// [`crate::queue::tombstone::on_transition`] write `fixed/<id>.json` or
/// `failed_again/<id>.json` when this job terminates, so "is the original
/// fixed?" stays a list diff instead of a rescan. `schedule_id` is
/// deliberately NOT carried: a manual rerun is not a scheduled submission,
/// and claiming otherwise would corrupt the schedule's own accounting.
fn rerun_options(original: &Job) -> SubmitOptions {
    let cpu_default = !original.gpu_mem_gb.is_positive()
        && original.gpu_type.is_empty()
        && original.machine_type == CPU_MACHINE_TYPE;
    let mut options = SubmitOptions {
        provider: original.provider.clone(),
        // A fresh `stado submit` opens a new batch; so does a rerun. The
        // original's batch belongs to the invocation that created it.
        batch_id: format!("batch-{}", chrono::Utc::now().timestamp()),
        bucket: crate::config::bucket().to_string(),
        preemptible: original.preemptible,
        max_cost_per_hour_usd: original.max_cost_per_hour_usd,
        pin_to_provider: original.pin_to_provider,
        priority: original.priority,
        repo: original.repo.clone(),
        repo_workdir: original.repo_workdir.clone(),
        repo_extras: original.repo_extras.clone(),
        pre_command: original.pre_command.clone(),
        apt_packages: original.apt_packages.clone(),
        output_uri: original.output_uri.clone(),
        verify_command: original.verify_command.clone(),
        exclusive: original.exclusive,
        re_submission_of: original.job_id.clone(),
        yieldable: original.yieldable,
        yield_command: original.yield_command.clone(),
        yield_grace_seconds: original.yield_grace_seconds,
        pinned_host: original.pinned_host.clone(),
        secret_env: original.secret_env.clone(),
        // The resolved map is the reproducible half: aliases were already
        // pinned to immutable versions at the original submit, and a rerun
        // must read the same bytes the original read.
        input_artifacts: original.input_artifacts.clone(),
        resolved_input_artifacts: original.resolved_input_artifacts.clone(),
        ..SubmitOptions::default()
    };
    if !cpu_default {
        options.gpu_type = original.gpu_type.clone();
        options.vram_gb = original.gpu_mem_gb;
        options.machine_type = original.machine_type.clone();
    }
    options
}

/// The fields a rerun has to reproduce, side by side, so "identical spec"
/// is something the operator can read off the terminal instead of trust.
/// The command is printed above the table rather than in it — it is
/// resubmitted verbatim, and a long one would pad every other row.
fn spec_rows(original: &Job, fresh: &Job) -> Vec<Vec<String>> {
    let row = |field: &str, was: String, now: String| vec![field.to_string(), was, now];
    vec![
        row("state", original.state.clone(), fresh.state.clone()),
        row(
            "provider",
            original.provider.clone(),
            fresh.provider.clone(),
        ),
        row(
            "gpu_mem_gb",
            original.gpu_mem_gb.to_string(),
            fresh.gpu_mem_gb.to_string(),
        ),
        row(
            "gpu_type",
            original.gpu_type.clone(),
            fresh.gpu_type.clone(),
        ),
        row(
            "machine_type",
            original.machine_type.clone(),
            fresh.machine_type.clone(),
        ),
        row(
            "priority",
            original.priority.to_string(),
            fresh.priority.to_string(),
        ),
        row(
            "preemptible",
            original.preemptible.to_string(),
            fresh.preemptible.to_string(),
        ),
        row(
            "exclusive",
            original.exclusive.to_string(),
            fresh.exclusive.to_string(),
        ),
        row(
            "pinned_host",
            original.pinned_host.clone(),
            fresh.pinned_host.clone(),
        ),
        row("run_id", original.run_id.clone(), fresh.run_id.clone()),
        row(
            "batch_id",
            original.batch_id.clone(),
            fresh.batch_id.clone(),
        ),
        row(
            "re_submission_of",
            original.re_submission_of.clone(),
            fresh.re_submission_of.clone(),
        ),
    ]
}

/// `stado job watch JOB_ID [--follow] [--json]` — print the log from the
/// start, then (with `--follow`) tail it until the job reaches a terminal
/// prefix and report the outcome.
async fn watch(job_id: &str, follow: bool, json: bool) -> Result<(), CmdError> {
    let facade = MachineFacade::new().await.map_err(cmd_error)?;
    let mut cursor = i64::default();
    let mut buffered = String::new();

    let job = loop {
        // State first, log second. A job observed in a terminal prefix has
        // already stopped writing, so the drain below cannot miss its last
        // bytes. The other order would read the log, watch the job go
        // terminal, and exit having dropped whatever landed in between.
        let job = facade.lookup_job(job_id).await.map_err(cmd_error)?;
        let terminal = job_state::is_terminal(&job.state);
        drain(&facade, job_id, &mut cursor, &mut buffered, json).await?;
        if terminal || !follow {
            break job;
        }
        tokio::time::sleep(WATCH_POLL_INTERVAL).await;
    };

    let terminal = job_state::is_terminal(&job.state);
    if json {
        let payload = json!({
            "job": normalize_job(&job),
            "terminal": terminal,
            "log_bytes": cursor,
            "log": buffered,
        });
        println!("{}", serde_json::to_string_pretty(&payload)?);
    } else {
        let ended = job.completed_at.clone().or_else(|| job.failed_at.clone());
        let row = vec![
            job.job_id.clone(),
            job.state.clone(),
            cursor.to_string(),
            job.started_at.clone().unwrap_or_default(),
            ended.unwrap_or_default(),
            job.error.clone().unwrap_or_default(),
        ];
        table::print(
            &["JOB", "STATE", "LOG BYTES", "STARTED", "ENDED", "ERROR"],
            std::slice::from_ref(&row),
        );
    }
    outcome(&job, terminal)
}

/// Print (or buffer) every byte past `cursor` and advance it, returning
/// once the page reports EOF — so a log that grew by more than one page
/// between polls still arrives whole in this poll.
///
/// The cursor is never reset between polls, which is the point: each read
/// asks for `[cursor, end)` and the stream stays monotone instead of
/// replaying the log every tick.
async fn drain(
    facade: &MachineFacade,
    job_id: &str,
    cursor: &mut i64,
    buffered: &mut String,
    buffer: bool,
) -> Result<(), CmdError> {
    loop {
        let page = match facade.read_logs(job_id, *cursor, LOG_PAGE_BYTES).await {
            Ok(page) => page,
            // An agent that restarts the command re-uploads the log from
            // the beginning, so a cursor that was valid a poll ago can end
            // up past the new end. Rewind and replay rather than dying
            // mid-tail. Guarded on a non-zero cursor: read_logs cannot
            // reject offset zero, so this can never spin.
            Err(exc) if exc.code == "INVALID_CURSOR" && *cursor != i64::default() => {
                *cursor = i64::default();
                eprintln!("-- log restarted from the beginning; rewinding --");
                continue;
            }
            Err(exc) => return Err(cmd_error(exc)),
        };
        let text = page["text"].as_str().unwrap_or_default();
        if buffer {
            buffered.push_str(text);
        } else {
            print!("{text}");
            // A tail that only flushes on newline stalls on a progress bar.
            let _ = std::io::stdout().flush();
        }
        *cursor = page["next_cursor"].as_i64().unwrap_or(*cursor);
        if page["eof"].as_bool().unwrap_or(true) {
            return Ok(());
        }
    }
}

/// The exit status carries the job's outcome, so
/// `stado job watch ID --follow && next-step` means what it reads like. A
/// job that is still running is not a failure — without `--follow` the
/// operator asked for a snapshot, not a verdict. In `--json` mode the
/// message goes to stderr and stdout stays a single parseable object.
fn outcome(job: &Job, terminal: bool) -> Result<(), CmdError> {
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
