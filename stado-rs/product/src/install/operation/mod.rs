//! The installation operation a product surface's lock stands for: which one
//! runs and in what step (`stado product status`), and the after-install
//! steps that run without the lock.
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
mod holders;
mod perform;
mod prerequisites;
mod supersede;
pub(in crate::install) use after::{after_install, lock_surface};
pub use perform::perform;
use supersede::{stop_after_install, stop_past_placement};

use anyhow::{Context, Result};
use serde_json::{json, Value};
use std::fs::{self, OpenOptions};
use std::path::Path;

use crate::common::{alive, process::birth::started_at, Runtime};
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
/// its pid and whether it runs, since when, its command and its step. A
/// record that cannot be read or parsed is said with its cause, and only an
/// empty one as the record of a Stado from before holders were recorded.
pub(crate) fn clause(path: &Path) -> String {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) => return format!("its holder record cannot be read: {error}"),
    };
    if text.trim().is_empty() {
        return "the holder recorded nothing (a Stado from before holders were recorded)".into();
    }
    let record = match serde_json::from_str::<Value>(text.trim()) {
        Ok(record) => record,
        Err(error) => return format!("its holder record is not one ({error}): {}", text.trim()),
    };
    let Some(pid) = record["pid"].as_u64() else {
        return "the holder record names no pid".into();
    };
    let field = |name: &str| match record[name].as_str() {
        Some(value) => value.to_owned(),
        None => format!("no recorded {name}"),
    };
    let state = match i32::try_from(pid) {
        Ok(pid) if alive(pid) => "running".to_owned(),
        Ok(_) => format!(
            "no longer running, so its record is stale; {}",
            holders::open_by(path)
        ),
        Err(error) => format!("a pid no process can have: {error}"),
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
    Ok(match post_placement(runtime, id, surface)? {
        Some(operation) => operation,
        None => Value::Null,
    })
}

/// The after-install steps the surface's receipt records as running
/// (`after_install_running`), with whether the process table still names the
/// recorded PID with the recorded birth time; `None` when the receipt records
/// none.
fn post_placement(runtime: &Runtime, id: &str, surface: &str) -> Result<Option<Value>> {
    let lock = state::path(runtime, id, surface)?.with_extension("lock");
    let Some(receipt) = ProductState::load(runtime, id, surface)? else {
        return Ok(None);
    };
    let Some(marker) = receipt.extra.get("after_install_running") else {
        return Ok(None);
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
    Ok(Some(report))
}
