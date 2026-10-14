//! The installation operation a product surface's lock stands for: which one
//! runs and in what step (`stado product status`), ending it through its
//! owner (`stado product cancel`), and the after-install steps that run
//! without the lock.
//!
//! An installation holds `<products>/<product>/<surface>.lock` while it
//! builds, places and records its files, and writes into it who holds it, in
//! which phase and step (`common::files`). Another installation of the same
//! surface, or one that depends on it, waits there naming that holder. What a
//! holder does after its files are placed and recorded (the catalog's
//! `after_install` steps: Stado's `release local converge` restarting the
//! units that run the replaced binary, which can wait on another host) does
//! not change the surface, so it runs without the lock: the receipt already
//! says `installed`, and the lock is taken back only to record the steps'
//! outcome, which is not written over a newer installation that placed files
//! meanwhile.

mod after;
mod cancel;
mod perform;
pub(in crate::install) use after::{after_install, lock_surface};
pub use cancel::cancel;
pub use perform::perform;

use anyhow::{Context, Result};
use serde_json::{json, Value};
use std::fs::{self, OpenOptions};
use std::path::Path;
use std::process::Command;

use crate::common::{alive, checked, process::started_at, Runtime};
use crate::state::{self, ProductState};

/// The record a held lock carries, with whether its holder still runs;
/// `None` when nothing holds the lock.
fn holder(lock: &Path) -> Result<Option<Value>> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    let file = match options.open(lock) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error).with_context(|| format!("{} cannot be opened", lock.display()))
        }
    };
    match file.try_lock_shared() {
        Ok(()) => {
            file.unlock()
                .with_context(|| format!("{} cannot be released", lock.display()))?;
            return Ok(None);
        }
        Err(std::fs::TryLockError::WouldBlock) => {}
        Err(std::fs::TryLockError::Error(error)) => {
            return Err(error).with_context(|| format!("inspecting {}", lock.display()))
        }
    }
    let record = crate::common::files::read_lock_record(lock)?
        .with_context(|| format!("{} is held but records no holder", lock.display()))?;
    let observed = record.observed_start()?;
    let running = record
        .started_at
        .as_ref()
        .is_some_and(|start| observed.as_ref() == Some(start));
    Ok(Some(json!({
        "product": record.product, "pid": record.pid,
        "started_at": record.started_at, "observed_start": observed,
        "running": running, "lock": lock, "command": record.command,
        "acquired_at": record.acquired_at, "phase": record.phase, "step": record.step,
    })))
}

/// The holder a lock file records, as one clause for a refusal or a wait:
/// its pid and whether it runs, since when, its command and its step.
pub(crate) fn clause(path: &Path) -> String {
    let Ok(text) = fs::read_to_string(path) else {
        return "its holder record cannot be read".into();
    };
    let Ok(record) = serde_json::from_str::<Value>(text.trim()) else {
        return "the holder recorded nothing (a Stado from before holders were recorded)".into();
    };
    let Some(pid) = record["pid"].as_u64() else {
        return "the holder record names no pid".into();
    };
    let field = |name: &str| match record[name].as_str() {
        Some(value) => value.to_owned(),
        None => format!("no recorded {name}"),
    };
    let state = match i32::try_from(pid).is_ok_and(alive) {
        true => "running",
        false => "no longer running",
    };
    let step = match record["step"].as_str() {
        Some(step) => format!(", in step {step}"),
        None => String::new(),
    };
    format!(
        "held by pid {pid} ({state}) since {}: {}{step}",
        field("acquired_at"),
        field("command")
    )
}

/// The locked writer or the post-placement step it recorded before releasing
/// the lock. A stale PID is not reported as a running installation.
pub fn running(runtime: &Runtime, id: &str, surface: &str) -> Result<Value> {
    let lock = state::path(runtime, id, surface)?.with_extension("lock");
    if let Some(held) = holder(&lock)? {
        return Ok(held);
    }
    let Some(receipt) = ProductState::load(runtime, id, surface)? else {
        return Ok(Value::Null);
    };
    let Some(marker) = receipt.extra.get("after_install_running") else {
        return Ok(Value::Null);
    };
    let pid = i32::try_from(
        marker["pid"]
            .as_i64()
            .context("after-install operation names no pid")?,
    )?;
    let expected = marker["started_at"]
        .as_str()
        .context("after-install operation records no process birth time")?;
    let observed = started_at(pid)?;
    let mut report = marker.clone();
    report["running"] = json!(observed.as_deref() == Some(expected));
    report["phase"] = json!("after_install");
    report["lock"] = json!(lock.display().to_string());
    Ok(report)
}

/// The direct children of `pid` with their command lines, as the process
/// table lists them now: the step an installation runs (a build, a
/// dependency's installation, an after-install reconciler).
fn children(pid: i32) -> Result<Vec<(i32, String)>> {
    let listed = checked(Command::new("/bin/ps").args(["-A", "-o", "pid=,ppid=,command="]))
        .context("the process table could not be read to find the operation's steps")?;
    let mut found = Vec::new();
    for line in String::from_utf8_lossy(&listed.stdout).lines() {
        let mut words = line.split_whitespace();
        let (Some(child), Some(parent)) = (words.next(), words.next()) else {
            continue;
        };
        let parent: i32 = parent
            .parse()
            .with_context(|| format!("ps printed a parent that is no pid: {line}"))?;
        if parent == pid {
            let child: i32 = child
                .parse()
                .with_context(|| format!("ps printed a pid that is no pid: {line}"))?;
            found.push((child, words.collect::<Vec<_>>().join(" ")));
        }
    }
    Ok(found)
}

/// Whether `command` runs the after-install step `step` (the recipe's argv):
/// its program is the step's program, and every literal word of the step
/// (not a placeholder, not an option, whose value may be one) follows in
/// order. A source installation leaves out an option whose value is a
/// release placeholder, so options are not required.
fn runs_step(command: &str, step: &Value) -> bool {
    let Some(words) = step.as_array() else {
        return false;
    };
    let mut ran = command.split_whitespace();
    let Some(program) = ran.next() else {
        return false;
    };
    let wanted: Vec<&str> = words.iter().filter_map(Value::as_str).collect();
    let Some((first, rest)) = wanted.split_first() else {
        return false;
    };
    if Path::new(program)
        .file_name()
        .and_then(|name| name.to_str())
        != Some(first)
    {
        return false;
    }
    let mut ran = ran.peekable();
    rest.iter()
        .filter(|word| !word.contains('{') && !word.starts_with("--"))
        .all(|word| {
            while ran.peek().is_some_and(|seen| seen != word) {
                ran.next();
            }
            ran.next().is_some()
        })
}

/// A holder from a Stado that records no end of placement keeps `placing`
/// while it runs its recipe's after-install steps, which do not change the
/// surface, so a newer installation waited on it for as long as such a step
/// stood (a converge reading another host). When the holder of `lock` says
/// `placing` and one of its children runs one of `recipe`'s `after_install`
/// steps, it is past placement: it and its children are told to stop, as a
/// holder still preparing is, and the newer installation takes the lock once
/// they let go. Each process told is said on stderr.
pub(super) fn stop_past_placement(lock: &Path, recipe: &Value) -> Result<()> {
    let Some(steps) = recipe["after_install"].as_array() else {
        return Ok(());
    };
    let Some(held) = holder(lock)? else {
        return Ok(());
    };
    if held["phase"].as_str() != Some(crate::common::PHASE_PLACING) || held["running"] != true {
        return Ok(());
    }
    let pid = i32::try_from(held["pid"].as_u64().context("the holder names no pid")?)?;
    let running = children(pid)?;
    let Some((_, step)) = running
        .iter()
        .find(|(_, command)| steps.iter().any(|step| runs_step(command, step)))
    else {
        return Ok(());
    };
    eprintln!(
        "superseding the installation {}: it is past placement, running its after-install step `{step}`",
        clause(lock)
    );
    for told in running
        .iter()
        .map(|(child, _)| *child)
        .chain([pid])
        .map(stop)
    {
        eprintln!("  {told}");
    }
    Ok(())
}

/// Tell `pid` to stop; the answer names the error when it could not be told.
fn stop(pid: i32) -> Value {
    // SAFETY: a signal to the holder the lock record names, or to a process
    // the process table lists as its child right now.
    let refused = unsafe { libc::kill(pid, libc::SIGTERM) }.is_negative();
    json!({"pid": pid, "told_to_stop": !refused,
        "error": refused.then(|| std::io::Error::last_os_error().to_string())})
}
