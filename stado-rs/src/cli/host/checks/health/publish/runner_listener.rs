//! Whether a running GitHub runner unit can take jobs.
//!
//! The init system sees only the wrapper a runner unit starts (`runsvc.sh`,
//! `start-runner.sh`, a reconcile script); the process that polls GitHub for
//! jobs is its child `bin/Runner.Listener`. On 2026-09-27 both wisent-backend
//! runners on charless-mac-mini were published `active` while one listener
//! never started (`Failed to create CoreCLR`) and the other had exited because
//! GitHub refused its runner version, so every deploy they serve queued and
//! `stado service list` called them healthy.
//!
//! A unit is a runner when the directory its program lives in (or the parent
//! of a `bin/` directory) holds GitHub's `.runner` registration file. For
//! such a unit the beacon asks the process table for that root's listener and,
//! when none runs, publishes `failed` with the newest runner log's last lines.

use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use crate::deploy::service::{STATE_ACTIVE, STATE_FAILED};

/// Lines of the newest runner log carried in the detail.
const LOG_TAIL_LINES: usize = 5;

/// The install root of the runner a unit program belongs to, when it is one.
/// `Err` carries a registration file the host would not let this account read.
fn runner_root(program: &str) -> Result<Option<PathBuf>, String> {
    let path = Path::new(program);
    let Some(mut root) = path.parent() else {
        return Ok(None);
    };
    if root.file_name().is_some_and(|name| name == "bin") {
        let Some(parent) = root.parent() else {
            return Ok(None);
        };
        root = parent;
    }
    let registration = root.join(".runner");
    match registration.try_exists() {
        Ok(true) => Ok(Some(root.to_path_buf())),
        Ok(false) => Ok(None),
        Err(error) => Err(format!(
            "{} could not be read: {error}",
            registration.display()
        )),
    }
}

/// Whether the process table holds this root's `bin/Runner.Listener`.
fn listener_running(root: &Path) -> Result<bool, String> {
    let listener = root.join("bin").join("Runner.Listener");
    let listener = listener.to_string_lossy();
    let table = crate::deploy::service::process_table()?;
    Ok(table.iter().any(|(_, _, argv)| {
        argv.strip_prefix(listener.as_ref())
            .is_some_and(|rest| rest.is_empty() || rest.starts_with(char::is_whitespace))
    }))
}

/// The last lines of the newest `_diag/Runner_*.log`, or why none was read.
fn newest_log_tail(root: &Path) -> String {
    let diag = root.join("_diag");
    let entries = match std::fs::read_dir(&diag) {
        Ok(entries) => entries,
        Err(error) => return format!("{} could not be read: {error}", diag.display()),
    };
    let newest = entries
        .filter_map(Result::ok)
        .filter(|entry| entry.file_name().to_string_lossy().starts_with("Runner_"))
        .filter_map(|entry| {
            let modified = entry.metadata().and_then(|meta| meta.modified()).ok()?;
            Some((modified, entry.path()))
        })
        .max_by_key(|(modified, _)| *modified);
    let Some((_, path)) = newest else {
        return format!("{} holds no Runner_*.log", diag.display());
    };
    match std::fs::read_to_string(&path) {
        Ok(text) => {
            let lines: Vec<&str> = text.lines().collect();
            let start = lines.len().saturating_sub(LOG_TAIL_LINES);
            format!("{} ends: {}", path.display(), lines[start..].join(" | "))
        }
        Err(error) => format!("{} could not be read: {error}", path.display()),
    }
}

/// Add one sentence to the entry's detail, after any the state read left.
fn add_detail(entry: &mut Map<String, Value>, sentence: String) {
    let detail = match entry.get("detail").and_then(Value::as_str) {
        Some(earlier) if !earlier.is_empty() => format!("{earlier}; {sentence}"),
        _ => sentence,
    };
    entry.insert("detail".to_string(), Value::String(detail));
}

/// Correct an `active` entry for a runner unit whose listener is not running.
/// Any other entry, and a unit that is not a runner, is left as it is.
pub(super) fn apply(entry: &mut Map<String, Value>, program: Option<&str>) {
    if entry.get("state").and_then(Value::as_str) != Some(STATE_ACTIVE) {
        return;
    }
    let Some(program) = program.filter(|program| Path::new(program).is_absolute()) else {
        return;
    };
    let root = match runner_root(program) {
        Ok(Some(root)) => root,
        Ok(None) => return,
        Err(detail) => {
            add_detail(
                entry,
                format!("whether this runner can take jobs is unread: {detail}"),
            );
            return;
        }
    };
    match listener_running(&root) {
        Ok(true) => {}
        Ok(false) => {
            entry.insert("state".to_string(), Value::String(STATE_FAILED.to_string()));
            add_detail(
                entry,
                format!(
                    "the unit runs, but no {}/bin/Runner.Listener is running, so GitHub delivers \
                     this runner no jobs; {}",
                    root.display(),
                    newest_log_tail(&root)
                ),
            );
        }
        Err(error) => add_detail(
            entry,
            format!(
                "whether this runner can take jobs is unread: the process table read failed: \
                 {error}"
            ),
        ),
    }
}
