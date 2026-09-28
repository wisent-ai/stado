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
//! A unit is a runner when its program's directory (or the parent of a `bin/`
//! directory), or one directory directly inside it, holds GitHub's `.runner`
//! registration file. The second shape is wisent-backend's release runner:
//! its launcher `reconcile-release-runner.sh` sits in the runner root and the
//! registered install is `vendor-<version>-layout-<n>/` beneath it. For such a
//! unit the beacon asks the process table for each install's listener and,
//! when none runs, publishes `failed` with each install's newest log lines.

use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use crate::deploy::service::{STATE_ACTIVE, STATE_FAILED};

/// Lines of the newest runner log carried in the detail.
const LOG_TAIL_LINES: usize = 5;

/// Whether `directory` holds a `.runner` registration file.
fn registered(directory: &Path) -> Result<bool, String> {
    let registration = directory.join(".runner");
    registration
        .try_exists()
        .map_err(|error| format!("{} could not be read: {error}", registration.display()))
}

/// Every registered runner install a unit program belongs to: the program's
/// own root and the directories directly inside it. Empty when the unit is
/// not a runner; `Err` names the directory or registration the host would not
/// let this account read.
fn runner_installs(program: &str) -> Result<Vec<PathBuf>, String> {
    let path = Path::new(program);
    let Some(mut root) = path.parent() else {
        return Ok(Vec::new());
    };
    if root.file_name().is_some_and(|name| name == "bin") {
        let Some(parent) = root.parent() else {
            return Ok(Vec::new());
        };
        root = parent;
    }
    if registered(root)? {
        return Ok(vec![root.to_path_buf()]);
    }
    let entries = std::fs::read_dir(root)
        .map_err(|error| format!("{} could not be listed: {error}", root.display()))?;
    let mut installs = Vec::new();
    for entry in entries {
        let entry =
            entry.map_err(|error| format!("{} could not be listed: {error}", root.display()))?;
        let candidate = entry.path();
        if candidate.is_dir() && registered(&candidate)? {
            installs.push(candidate);
        }
    }
    installs.sort();
    Ok(installs)
}

/// Whether the process table holds any of these installs' `bin/Runner.Listener`.
fn listener_running(installs: &[PathBuf]) -> Result<bool, String> {
    let listeners: Vec<String> = installs
        .iter()
        .map(|install| {
            install
                .join("bin")
                .join("Runner.Listener")
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    let table = crate::deploy::service::process_table()?;
    Ok(table.iter().any(|(_, _, argv)| {
        listeners.iter().any(|listener| {
            argv.strip_prefix(listener.as_str())
                .is_some_and(|rest| rest.is_empty() || rest.starts_with(char::is_whitespace))
        })
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
    let installs = match runner_installs(program) {
        Ok(installs) if installs.is_empty() => return,
        Ok(installs) => installs,
        Err(detail) => {
            add_detail(
                entry,
                format!("whether this unit is a GitHub runner could not be read: {detail}"),
            );
            return;
        }
    };
    match listener_running(&installs) {
        Ok(true) => {}
        Ok(false) => {
            entry.insert("state".to_string(), Value::String(STATE_FAILED.to_string()));
            let listeners: Vec<String> = installs
                .iter()
                .map(|install| format!("{}/bin/Runner.Listener", install.display()))
                .collect();
            let tails: Vec<String> = installs
                .iter()
                .map(|install| newest_log_tail(install.as_path()))
                .collect();
            add_detail(
                entry,
                format!(
                    "the unit runs, but no {} is running, so GitHub delivers this runner no \
                     jobs; {}",
                    listeners.join(" or "),
                    tails.join("; ")
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
