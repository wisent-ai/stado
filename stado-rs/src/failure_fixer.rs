//! Autonomous failure-fixer: failure -> local Claude Code CLI -> ship fix -> retry.
//!
//! Port of `stado/failure_fixer/__init__.py` + `stado/failure_fixer/cli.py`
//! ([`cli_main`]). The loop, per the operator's spec:
//!   1. A job fails (lands in `failed/<jid>.json`)
//!   2. [`scan_new_failures`] picks it up
//!   3. [`dispatch_fix`] exec's the local `claude` CLI with the fix prompt
//!   4. Claude Code diagnoses, ships the fix to PyPI, resubmits
//!   5. Per-job state at `failure_fixes/<jid>.json` so the same job is not
//!      re-dispatched on subsequent scans
//!
//! One dispatch per failed job_id. No fingerprint clustering.
//!
//! Authentication is resolved from `stado-anthropic/api_key` through the
//! control-plane Skarbiec grant and injected only into the Claude child.
//!
//! STALE DOCS NOTE (ported faithfully from `failure_fixer/cli.py`): the
//! Python CLI's help strings still say "HMAC-sign + POST to model-router"
//! but the implementation execs the local `claude` CLI. This port follows
//! the IMPLEMENTATION; the stale help text is preserved on the clap flags.
//!
//! After FAILURE_FIXER_ATTEMPT_CAP attempts on the same job the job is
//! marked EXHAUSTED and stops being re-dispatched so a permanently-broken
//! job does not burn unlimited Claude session budget.

use serde_json::{json, Map, Value};

use crate::config;
use crate::models::{json_dumps_pretty_sorted, py_str_repr};
use crate::queue::{JobStorage, StorageError};

/// Python `EXHAUSTED` — attempt cap reached, stop re-dispatching.
pub const EXHAUSTED: &str = "exhausted";
/// Python `DISPATCHED` — `claude -p` exited 0.
pub const DISPATCHED: &str = "dispatched";
/// Python `DISPATCH_FAILED` — `claude -p` exited nonzero.
pub const DISPATCH_FAILED: &str = "dispatch_failed";
/// Python `DRY_RUN` — no `--execute`; payload returned without exec'ing.
pub const DRY_RUN: &str = "dry_run";
/// Python `ALREADY_DISPATCHED` — state shows attempts>0, skipped by scan.
pub const ALREADY_DISPATCHED: &str = "already_dispatched";
/// Python `CLAUDE_NOT_FOUND` — `claude` CLI not on PATH.
pub const CLAUDE_NOT_FOUND: &str = "claude_cli_not_found";

/// Fixer-layer error (storage + the JSON state files).
#[derive(Debug, thiserror::Error)]
pub enum FixError {
    #[error(transparent)]
    Storage(#[from] StorageError),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Skarbiec(#[from] crate::skarbiec::SkarbiecError),
}

/// One failed job's relevant fields, parsed from `failed/<jid>.json`
/// (Python `FailureRecord`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FailureRecord {
    pub job_id: String,
    pub batch_id: String,
    pub command: String,
    pub error: String,
    pub failed_at: String,
}

/// Python `s[:n]` on a `str` (character-based).
fn truncate_chars(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

/// Python `_parse_failed_blob`: absent/corrupt blobs become None; missing
/// fields default to "".
async fn parse_failed_blob(
    store: &JobStorage,
    name: &str,
) -> Result<Option<FailureRecord>, FixError> {
    let Some(txt) = store.download_text(name).await? else {
        return Ok(None);
    };
    let Ok(blob) = serde_json::from_str::<Value>(&txt) else {
        return Ok(None);
    };
    let field = |key: &str| {
        blob.get(key)
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string()
    };
    Ok(Some(FailureRecord {
        job_id: field("job_id"),
        batch_id: field("batch_id"),
        command: field("command"),
        error: field("error"),
        failed_at: field("failed_at"),
    }))
}

/// Python `scan_new_failures`: every `failed/<jid>.json` whose failed_at
/// is >= `since_iso` (ISO-8601 lexicographic compare) AND whose command
/// contains `command_pattern` (case-sensitive substring) if set. None
/// means "no filter" for both.
pub async fn scan_new_failures(
    store: &JobStorage,
    since_iso: Option<&str>,
    command_pattern: Option<&str>,
) -> Result<Vec<FailureRecord>, FixError> {
    let mut out = Vec::new();
    for info in store.list_blobs_with_meta("failed/").await? {
        if !info.name.ends_with(".json") {
            continue;
        }
        let Some(rec) = parse_failed_blob(store, &info.name).await? else {
            continue;
        };
        if let Some(since) = since_iso {
            if rec.failed_at.as_str() < since {
                continue;
            }
        }
        if let Some(pattern) = command_pattern {
            if !rec.command.contains(pattern) {
                continue;
            }
        }
        out.push(rec);
    }
    Ok(out)
}

fn state_path(job_id: &str) -> String {
    format!("{}/{job_id}.json", config::FAILURE_FIXER_STATE_PREFIX)
}

/// Python `state_load`: `{}` when the state blob is absent.
pub async fn state_load(store: &JobStorage, job_id: &str) -> Result<Value, FixError> {
    let Some(txt) = store.download_text(&state_path(job_id)).await? else {
        return Ok(Value::Object(Map::new()));
    };
    Ok(serde_json::from_str(&txt)?)
}

/// Python `state_save`: `json.dumps(state, indent=2, sort_keys=True)`.
pub async fn state_save(store: &JobStorage, job_id: &str, state: &Value) -> Result<(), FixError> {
    store
        .upload_text(&state_path(job_id), &json_dumps_pretty_sorted(state))
        .await?;
    Ok(())
}

/// Build the structured prompt passed to the local Claude Code CLI for ONE
/// failed job. Byte-identical to Python `format_fix_prompt`.
pub fn format_fix_prompt(rec: &FailureRecord, max_error_chars: usize) -> String {
    let err = truncate_chars(&rec.error, max_error_chars);
    format!(
        "You are the wisent-compute autonomous failure-fixer.\n\
         A wisent-compute job (job_id={} batch_id={}) failed at {}.\n\
         \n\
         Diagnose the root cause from the traceback below, ship the fix to \
         the appropriate repo (wisent / wisent-tools / wisent-compute), \
         publish the patched package to PyPI, then resubmit this exact \
         command via `wc submit <command> --verify <verify_command>`. The \
         fleet's local agents drift-pick-up the new PyPI release on their \
         next loop; cloud agents self-terminate on drift so a fresh VM with \
         the new version claims the resubmitted job.\n\
         \n\
         Failed command:\n  {}\n\
         \n\
         Traceback (last {} chars of stderr):\n\
         ---BEGIN TRACEBACK---\n{}\n---END TRACEBACK---\n\
         \n\
         Constraints: never introduce mocks, soft-defaults, or silent \
         error absorption. Diagnose root cause and patch the cause; if \
         the root is in an upstream dependency the wisent-compute team \
         cannot patch, surface that clearly instead of inventing a \
         workaround.",
        rec.job_id, rec.batch_id, rec.failed_at, rec.command, max_error_chars, err
    )
}

/// [`format_fix_prompt`] with the configured error cap
/// (`FAILURE_FIX_PROMPT_ERROR_BYTES`).
pub fn format_fix_prompt_default(rec: &FailureRecord) -> String {
    format_fix_prompt(rec, config::FAILURE_FIX_PROMPT_ERROR_BYTES as usize)
}

/// Python `_claude_bin`: locate the local `claude` CLI on PATH
/// (`shutil.which`). None when absent or not executable.
pub fn claude_bin() -> Option<String> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join("claude");
        if candidate.is_file() && is_executable(&candidate) {
            return Some(candidate.to_string_lossy().into_owned());
        }
    }
    None
}

#[cfg(unix)]
fn is_executable(path: &std::path::Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    path.metadata()
        .map(|meta| meta.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable(path: &std::path::Path) -> bool {
    path.is_file()
}

/// Python `dispatch_fix`: exec the local `claude` CLI with the fix prompt
/// for ONE failed job. Returns the dispatch record dict (JSON). When
/// `execute` is false, returns the dispatch payload without exec'ing.
pub async fn dispatch_fix(
    rec: &FailureRecord,
    store: &JobStorage,
    execute: bool,
) -> Result<Value, FixError> {
    let mut state = state_load(store, &rec.job_id).await?;
    let attempts = state.get("attempts").and_then(Value::as_i64).unwrap_or(0);
    if attempts >= config::FAILURE_FIXER_ATTEMPT_CAP {
        return Ok(json!({"job_id": rec.job_id, "status": EXHAUSTED, "attempts": attempts}));
    }
    let prompt = format_fix_prompt_default(rec);
    let claude = claude_bin();
    if !execute {
        return Ok(json!({
            "job_id": rec.job_id,
            "status": DRY_RUN,
            "attempts": attempts,
            "claude_bin": claude.clone().unwrap_or_else(|| "(not found on PATH)".to_string()),
            "prompt_bytes": prompt.chars().count(),
            "prompt_preview": truncate_chars(&prompt, 500),
        }));
    }
    let Some(claude) = claude else {
        return Ok(json!({
            "job_id": rec.job_id,
            "status": CLAUDE_NOT_FOUND,
            "attempts": attempts,
            "error": "`claude` CLI not on PATH. Install Claude Code before \
                      running the failure-fixer.",
        }));
    };
    let anthropic_key = crate::skarbiec::read_string("stado-anthropic", "api_key")
        .await?
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            crate::skarbiec::SkarbiecError::MissingValue("stado-anthropic/api_key".into())
        })?;
    // The CLI receives its credential from Skarbiec for this child only.
    let proc = std::process::Command::new(&claude)
        .arg("-p")
        .arg(&prompt)
        .env("ANTHROPIC_API_KEY", anthropic_key)
        .output()?;
    let rc = proc.status.code().unwrap_or(-1);
    let stdout = String::from_utf8_lossy(&proc.stdout);
    let stderr = String::from_utf8_lossy(&proc.stderr);
    if !state.is_object() {
        state = Value::Object(Map::new());
    }
    let slot = state.as_object_mut().expect("ensured object");
    slot.insert("attempts".into(), Value::from(attempts + 1));
    slot.insert(
        "last_dispatched_at".into(),
        Value::from(chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()),
    );
    slot.insert("last_returncode".into(), Value::from(rc));
    slot.insert(
        "last_stdout_preview".into(),
        Value::from(truncate_chars(&stdout, 600)),
    );
    slot.insert(
        "last_stderr_preview".into(),
        Value::from(truncate_chars(&stderr, 600)),
    );
    slot.insert("failed_at".into(), Value::from(rec.failed_at.as_str()));
    slot.insert("batch_id".into(), Value::from(rec.batch_id.as_str()));
    slot.insert("command".into(), Value::from(rec.command.as_str()));
    state_save(store, &rec.job_id, &state).await?;
    Ok(json!({
        "job_id": rec.job_id,
        "status": if rc == 0 { DISPATCHED } else { DISPATCH_FAILED },
        "attempts": attempts + 1,
        "returncode": rc,
        "stdout_preview": truncate_chars(&stdout, 300),
    }))
}

/// Python `scan_and_dispatch`: scan failed/ -> exec local `claude` per
/// UNHANDLED failed job. `skip_dispatched` reads state and skips jobs
/// whose state file already shows attempts>0 (recording them as
/// ALREADY_DISPATCHED). Returns the per-job dispatch records.
pub async fn scan_and_dispatch(
    since_iso: Option<&str>,
    command_pattern: Option<&str>,
    execute: bool,
    store: &JobStorage,
    skip_dispatched: bool,
) -> Result<Vec<Value>, FixError> {
    let mut out = Vec::new();
    for rec in scan_new_failures(store, since_iso, command_pattern).await? {
        if skip_dispatched {
            let prior = state_load(store, &rec.job_id).await?;
            let attempts = prior.get("attempts").and_then(Value::as_i64).unwrap_or(0);
            if attempts > 0 {
                out.push(json!({
                    "job_id": rec.job_id,
                    "status": ALREADY_DISPATCHED,
                    "attempts": attempts,
                }));
                continue;
            }
        }
        out.push(dispatch_fix(&rec, store, execute).await?);
    }
    Ok(out)
}

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

