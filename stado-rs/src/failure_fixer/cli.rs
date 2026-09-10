//! The `stado-fix` command group: list the failures that landed, print the
//! prompt one of them would send, and dispatch that prompt for real.

use serde_json::{json, Value};

use crate::config;
use crate::models::py_str_repr;
use crate::queue::JobStorage;

use super::{
    dispatch_fix, format_fix_prompt_default, scan_new_failures, state_load, truncate_chars,
    scan_and_dispatch, FixError,
};

// ---------------------------------------------------------------------------
// failure_fixer/cli.py — `stado-fix` click group
// ---------------------------------------------------------------------------

/// Print `value` as Python `json.dumps(value, indent=2)` (insertion
/// order) on stdout.
fn print_pretty(value: &Value) {
    let pretty = serde_json::to_string_pretty(value).expect("JSON serialization is infallible");
    println!("{}", crate::models::ensure_ascii(&pretty));
}

#[derive(clap::Parser)]
#[command(
    name = "stado-fix",
    about = "Autonomous failure-fixer: failure -> Claude Code -> ship fix -> retry."
)]
struct Cli {
    #[command(subcommand)]
    command: FixCommands,
}

#[derive(clap::Subcommand)]
enum FixCommands {
    /// List recent failed jobs and their dispatch state.
    Scan {
        /// ISO-8601 lower bound on failed_at
        #[arg(long)]
        since: Option<String>,
    },
    /// Emit the Claude Code fix prompt for one failed job to stdout.
    Prompt {
        job_id: String,
        #[arg(long)]
        since: Option<String>,
    },
    /// Dispatch a fix request for ONE failed job to Claude Code via
    /// model-router. Writes per-job state to
    /// gs://<bucket>/failure_fixes/<job_id>.json.
    Dispatch {
        job_id: String,
        #[arg(long)]
        since: Option<String>,
        // STALE HELP TEXT (ported from Python): the implementation execs
        // the local `claude` CLI; there is no model-router POST.
        /// Actually HMAC-sign + POST to model-router; default dry-run.
        #[arg(long)]
        execute: bool,
    },
    /// Scan failed/ and dispatch one Claude Code session per undispatched
    /// job. Per-job ATTEMPT_CAP stops re-dispatching after
    /// FAILURE_FIXER_ATTEMPT_CAP attempts.
    ScanDispatch {
        #[arg(long)]
        since: Option<String>,
        /// Only dispatch failures whose command contains this substring
        /// (e.g. 'raw.extract_and_upload'). Without this the scan touches
        /// every failed/ blob and burns Claude OAuth quota on historical
        /// failures the operator does not care about.
        #[arg(long = "command-pattern")]
        command_pattern: Option<String>,
        /// Actually dispatch each undispatched failed job; default dry-run.
        #[arg(long)]
        execute: bool,
    },
}

/// The `stado-fix` entry point (click group). Exit codes match click: 2
/// for usage errors (clap parse failures), 1 for runtime failures and a
/// job_id with no failed/ blob, 0 on success.
pub async fn cli_main() -> i32 {
    let cli = <Cli as clap::Parser>::parse();
    let run = run_inner(cli.command).await;
    match run {
        Ok(code) => code,
        Err(err) => {
            eprintln!("Error: {err}");
            1
        }
    }
}

async fn run_inner(command: FixCommands) -> Result<i32, FixError> {
    let store = JobStorage::with_bucket(config::bucket()).await?;
    match command {
        FixCommands::Scan { since } => {
            let records = scan_new_failures(&store, since.as_deref(), None).await?;
            let mut summary = Vec::with_capacity(records.len());
            for rec in &records {
                let state = state_load(&store, &rec.job_id).await?;
                let attempts = state.get("attempts").and_then(Value::as_i64).unwrap_or(0);
                summary.push(json!({
                    "job_id": rec.job_id,
                    "batch_id": rec.batch_id,
                    "failed_at": rec.failed_at,
                    "attempts": attempts,
                    "command_head": truncate_chars(&rec.command, 160),
                }));
            }
            let undispatched = summary
                .iter()
                .filter(|s| s["attempts"].as_i64() == Some(0))
                .count();
            print_pretty(&json!({
                "total_failures_scanned": records.len(),
                "undispatched": undispatched,
                "already_dispatched": records.len() - undispatched,
                "jobs": Value::Array(summary),
            }));
            Ok(0)
        }
        FixCommands::Prompt { job_id, since } => {
            for rec in scan_new_failures(&store, since.as_deref(), None).await? {
                if rec.job_id == job_id {
                    println!("{}", format_fix_prompt_default(&rec));
                    return Ok(0);
                }
            }
            eprintln!("no failed job {} in current failed/", py_str_repr(&job_id));
            Ok(1)
        }
        FixCommands::Dispatch {
            job_id,
            since,
            execute,
        } => {
            for rec in scan_new_failures(&store, since.as_deref(), None).await? {
                if rec.job_id == job_id {
                    let result = dispatch_fix(&rec, &store, execute).await?;
                    print_pretty(&result);
                    return Ok(0);
                }
            }
            eprintln!("no failed job {} in current failed/", py_str_repr(&job_id));
            Ok(1)
        }
        FixCommands::ScanDispatch {
            since,
            command_pattern,
            execute,
        } => {
            let results = scan_and_dispatch(
                since.as_deref(),
                command_pattern.as_deref(),
                execute,
                &store,
                true,
            )
            .await?;
            print_pretty(&json!({"results": results, "count": results.len()}));
            Ok(0)
        }
    }
}
