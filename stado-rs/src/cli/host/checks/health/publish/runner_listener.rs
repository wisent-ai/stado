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
//! The runner install is taken from the unit's own launch chain, never from
//! what happens to sit near its program:
//! - the program's directory (or the parent of its `bin/`) holds GitHub's
//!   `.runner` registration, as for `runsvc.sh` and `start-runner.sh`; or
//! - the launcher names its install in `.stado-runner-install` beside itself.
//!   wisent-backend's release runner is this shape: `reconcile-release-runner.sh`
//!   sits in the runner root, selects `vendor-<version>-layout-<n>/`, records
//!   it there, changes into it and `exec`s `run.sh`. The record outlives the
//!   wrapper, which exits 0 when the listener stops for good; or
//! - the unit's running process works in a registered directory inside the
//!   program's directory, for a launcher that keeps no such record.
//!
//! For such a unit the beacon asks the process table for that install's
//! listener and, when none runs, publishes `failed` with the newest runner
//! log's last lines.

use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use crate::deploy::service::{STATE_ACTIVE, STATE_FAILED};

/// Lines of the newest runner log carried in the detail.
const LOG_TAIL_LINES: usize = 5;

/// The file a runner launcher writes beside itself naming the install it ran.
const INSTALL_MARKER: &str = ".stado-runner-install";

/// Whether `directory` holds a `.runner` registration file.
fn registered(directory: &Path) -> Result<bool, String> {
    let registration = directory.join(".runner");
    registration
        .try_exists()
        .map_err(|error| format!("{} could not be read: {error}", registration.display()))
}

/// The directory a process works in.
#[cfg(target_os = "linux")]
fn process_cwd(pid: &str) -> Result<PathBuf, String> {
    let link = format!("/proc/{pid}/cwd");
    std::fs::read_link(&link).map_err(|error| format!("{link} could not be read: {error}"))
}

/// The directory a process works in.
#[cfg(not(target_os = "linux"))]
fn process_cwd(pid: &str) -> Result<PathBuf, String> {
    let output = std::process::Command::new("/usr/sbin/lsof")
        .args(["-a", "-p", pid, "-d", "cwd", "-Fn"])
        .output()
        .map_err(|error| format!("lsof did not run for pid {pid}: {error}"))?;
    let listing = String::from_utf8_lossy(&output.stdout);
    listing
        .lines()
        .find_map(|line| line.strip_prefix('n'))
        .map(PathBuf::from)
        .ok_or_else(|| {
            format!(
                "lsof named no working directory for pid {pid} (exit {}): {}",
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            )
        })
}

/// The registered runner install this unit launches, when it launches one.
/// `Err` names the registration or process the host would not let this
/// account read.
fn runner_install(program: &str, pid: Option<&str>) -> Result<Option<PathBuf>, String> {
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
    if registered(root)? {
        return Ok(Some(root.to_path_buf()));
    }
    // A launcher that selects an install records it, so a unit whose wrapper
    // already exited (run.sh returns 0 when the listener stops for good) is
    // still tied to the install it ran.
    let marker = root.join(INSTALL_MARKER);
    match std::fs::read_to_string(&marker) {
        Ok(text) => {
            let chosen = PathBuf::from(text.trim());
            return if chosen.starts_with(root) && chosen != root {
                Ok(Some(chosen))
            } else {
                Err(format!(
                    "{} names {}, which is not an install inside {}",
                    marker.display(),
                    chosen.display(),
                    root.display()
                ))
            };
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(format!("{} could not be read: {error}", marker.display())),
    }
    let Some(pid) = pid.filter(|pid| pid.parse::<u32>().is_ok_and(|pid| pid > 0)) else {
        return Ok(None);
    };
    // Only a program directory that holds registered installs can launch one;
    // everywhere else the unit's working directory is not asked for at all.
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
    if installs.is_empty() {
        return Ok(None);
    }
    // Which of them this unit runs is what its process works in.
    let cwd = process_cwd(pid)?;
    Ok(installs
        .into_iter()
        .find(|install| *install == cwd || install.canonicalize().is_ok_and(|real| real == cwd)))
}

/// Whether the process table holds this install's `bin/Runner.Listener`.
fn listener_running(install: &Path) -> Result<bool, String> {
    let listener = install.join("bin").join("Runner.Listener");
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
pub(super) fn apply(entry: &mut Map<String, Value>, program: Option<&str>, pid: Option<&str>) {
    if entry.get("state").and_then(Value::as_str) != Some(STATE_ACTIVE) {
        return;
    }
    let Some(program) = program.filter(|program| Path::new(program).is_absolute()) else {
        return;
    };
    let install = match runner_install(program, pid) {
        Ok(Some(install)) => install,
        Ok(None) => return,
        Err(detail) => {
            add_detail(
                entry,
                format!("whether this unit is a GitHub runner could not be read: {detail}"),
            );
            return;
        }
    };
    match listener_running(&install) {
        Ok(true) => {}
        Ok(false) => {
            entry.insert("state".to_string(), Value::String(STATE_FAILED.to_string()));
            add_detail(
                entry,
                format!(
                    "the unit runs, but no {}/bin/Runner.Listener is running, so GitHub delivers \
                     this runner no jobs; {}",
                    install.display(),
                    newest_log_tail(&install)
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
