//! The janitor's owner-controlled state file and the records it carries.

pub(crate) mod error;
pub(crate) mod report;
pub(crate) mod write;

use std::fs::OpenOptions;
use std::io::{self, Read};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::Path;

use serde_json::{Map, Value};

use crate::providers::local::disk_cleanup::janitor::pass::lock::euid;
use crate::providers::local::disk_cleanup::janitor::state::error::JanitorError;
use crate::providers::local::disk_cleanup::janitor::{STATE_NAME, WRITER_ATTEMPTS};

// ---------------------------------------------------------------------------
// state file read / write
// ---------------------------------------------------------------------------

/// Python `_read_state`: owner-controlled, no-follow, plain-dict JSON.
pub(crate) fn read_state(state_dir: &Path) -> Result<Value, JanitorError> {
    let path = state_dir.join(STATE_NAME);
    let file = match OpenOptions::new()
        .read(true)
        .custom_flags(nix::libc::O_NOFOLLOW)
        .open(&path)
    {
        Ok(file) => file,
        Err(exc) if exc.kind() == io::ErrorKind::NotFound => return Ok(Value::Object(Map::new())),
        Err(exc) => return Err(exc.into()),
    };
    let info = file.metadata()?;
    if !info.is_file() || info.uid() != euid() {
        return Err(JanitorError::os("unsafe cleanup state"));
    }
    let mut text = String::new();
    (&file).read_to_string(&mut text)?;
    let value: Value = serde_json::from_str(&text)?;
    Ok(if value.is_object() {
        value
    } else {
        Value::Object(Map::new())
    })
}

/// One writer's own last attempt, or `None` when it has never recorded one.
///
/// `None` means run: a writer that has never stamped the file has no interval
/// to be inside. That is also the upgrade path - the first pass by each writer
/// after this change runs once immediately, because the old file carries only
/// the shared `last_attempt_at`.
pub(crate) fn writer_last_attempt(state: &Value, writer: &str) -> Option<f64> {
    state
        .get(WRITER_ATTEMPTS)
        .and_then(Value::as_object)
        .and_then(|stamps| stamps.get(writer))
        .and_then(Value::as_f64)
}
/// Policy identity of unfinished reclaim work. The report fallback migrates a
/// pre-intent `cap_reached` state without treating arbitrary stale cursors as
/// resumable.
pub(crate) fn reclaim_intent_digest(state: &Value) -> Option<&str> {
    state
        .get("reclaim_intent")
        .and_then(|intent| intent.get("policy_digest"))
        .and_then(Value::as_str)
        .or_else(|| {
            let report = state.get("report")?;
            (report.get("outcome").and_then(Value::as_str) == Some("cap_reached")
                && report.get("pressure_active").and_then(Value::as_bool) == Some(true))
            .then(|| report.get("policy_digest").and_then(Value::as_str))
            .flatten()
        })
}
pub(crate) fn reclaim_intent_outcome(state: &Value) -> Option<&str> {
    state
        .get("reclaim_intent")
        .and_then(|intent| intent.get("outcome"))
        .and_then(Value::as_str)
        .or_else(|| {
            let report = state.get("report")?;
            (report.get("outcome").and_then(Value::as_str) == Some("cap_reached")
                && report.get("pressure_active").and_then(Value::as_bool) == Some(true))
            .then_some("cap_reached")
        })
}
#[derive(Debug, Clone, Copy)]
pub(crate) enum ControlUpdateAuthority {
    /// This report is from the process admitted by the exclusive run lock.
    Owner,
    /// This report is diagnostic-only and must not mutate scan control.
    Preserve,
}
