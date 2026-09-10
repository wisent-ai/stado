//! Handing one failure to the local Claude Code CLI: the prompt it is given,
//! where that binary is found, and what the attempt records about itself.

use serde_json::{json, Map, Value};

use super::{
    scan_new_failures, state_load, state_save, truncate_chars, FailureRecord, FixError,
    ALREADY_DISPATCHED,
    CLAUDE_NOT_FOUND, DISPATCHED, DISPATCH_FAILED, DRY_RUN, EXHAUSTED,
};
use crate::config;
use crate::queue::JobStorage;

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
