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
//! A failure is dispatched once. The job's state file records the
//! `failed_at` of the failure its last session was given, and a scan that
//! finds the same failure again answers `already_dispatched`; only a new
//! failure of the job — a later `failed_at` — earns another session. A
//! permanently broken job therefore costs one session per time it fails, not
//! a count somebody picked.

use serde_json::{Map, Value};

use crate::config;
use crate::models::json_dumps_pretty_sorted;
use crate::queue::{JobStorage, StorageError};

/// Python `DISPATCHED` — `claude -p` exited 0.
pub const DISPATCHED: &str = "dispatched";
/// Python `DISPATCH_FAILED` — `claude -p` exited nonzero.
pub const DISPATCH_FAILED: &str = "dispatch_failed";
/// Python `DRY_RUN` — no `--execute`; payload returned without exec'ing.
pub const DRY_RUN: &str = "dry_run";
/// The failure in hand already had its session: the job's state records
/// this same `failed_at`.
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

mod cli;
mod dispatch;

pub use cli::cli_main;
pub(crate) use cli::run_resident;
pub use dispatch::{claude_bin, dispatch_fix, format_fix_prompt, scan_and_dispatch};
