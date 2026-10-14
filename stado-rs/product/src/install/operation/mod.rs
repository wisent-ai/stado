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

use anyhow::{bail, Context, Result};
use fs2::FileExt;
use serde_json::{json, Value};
use stado_wait as wait;
use std::fs::{self, File, OpenOptions};
use std::path::Path;
use std::process::Command;

use crate::common::{alive, checked, now, Runtime};
use crate::state::{self, ProductState};

/// The record a held lock carries, with whether its holder still runs;
/// `None` when nothing holds the lock.
fn holder(lock: &Path) -> Result<Option<Value>> {
    let file = match OpenOptions::new().read(true).open(lock) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error).with_context(|| format!("{} cannot be opened", lock.display()))
        }
    };
    // A shared hold this process can take means no installation holds the
    // exclusive lock; it is given back at once.
    if file.try_lock_shared().is_ok() {
        file.unlock()
            .with_context(|| format!("{} cannot be released", lock.display()))?;
        return Ok(None);
    }
    let text =
        fs::read_to_string(lock).with_context(|| format!("{} cannot be read", lock.display()))?;
    let mut record: Value = serde_json::from_str(text.trim()).with_context(|| {
        format!(
            "{} is held but records no readable holder: {}",
            lock.display(),
            text.trim()
        )
    })?;
    let pid = record["pid"]
        .as_u64()
        .with_context(|| format!("{} is held but names no pid", lock.display()))?;
    let pid = i32::try_from(pid)
        .with_context(|| format!("{} names pid {pid}, which is no process id", lock.display()))?;
    record["running"] = json!(alive(pid));
    record["lock"] = json!(lock.display().to_string());
    Ok(Some(record))
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

/// The installation operation holding `id`'s `surface`, or null.
pub fn running(runtime: &Runtime, id: &str, surface: &str) -> Result<Value> {
    let lock = state::path(runtime, id, surface)?.with_extension("lock");
    Ok(json!(holder(&lock)?))
}

/// The direct children of `pid`, as the process table lists them now: the
/// step an installation runs (a build, a dependency's installation).
fn children(pid: i32) -> Result<Vec<i32>> {
    let listed = checked(Command::new("/bin/ps").args(["-A", "-o", "pid=,ppid="]))
        .context("the process table could not be read to find the operation's steps")?;
    let mut found = Vec::new();
    for line in String::from_utf8_lossy(&listed.stdout).lines() {
        let words: Vec<&str> = line.split_whitespace().collect();
        if let [child, parent] = words[..] {
            let parent: i32 = parent
                .parse()
                .with_context(|| format!("ps printed a parent that is no pid: {line}"))?;
            if parent == pid {
                found.push(
                    child
                        .parse()
                        .with_context(|| format!("ps printed a pid that is no pid: {line}"))?,
                );
            }
        }
    }
    Ok(found)
}

/// Tell `pid` to stop; the answer names the error when it could not be told.
fn stop(pid: i32) -> Value {
    // SAFETY: a signal to the holder the lock record names, or to a process
    // the process table lists as its child right now.
    let refused = unsafe { libc::kill(pid, libc::SIGTERM) }.is_negative();
    json!({"pid": pid, "told_to_stop": !refused,
        "error": refused.then(|| std::io::Error::last_os_error().to_string())})
}

/// End the installation operation holding `id`'s `surface`: its owner and
/// the step it runs are told to stop (SIGTERM), the lock is waited for until
/// the owner has let it go, and the receipt records the cancelled operation
/// with its step. A surface nothing holds is refused by name.
pub fn cancel(runtime: &Runtime, id: &str, surface: &str) -> Result<Value> {
    let lock = state::path(runtime, id, surface)?.with_extension("lock");
    let Some(held) = holder(&lock)? else {
        bail!(
            "no installation operation holds {id}/{surface} ({}); nothing was cancelled",
            lock.display()
        );
    };
    let pid = i32::try_from(held["pid"].as_u64().context("the holder names no pid")?)?;
    let signalled: Vec<Value> = children(pid)?.into_iter().chain([pid]).map(stop).collect();
    let file = File::open(&lock).with_context(|| format!("{} cannot be opened", lock.display()))?;
    wait::blocking(
        wait::Kind::Lock,
        format!("the cancelled installation operation pid {pid} to let go of its lock"),
        lock.display(),
        || file.lock_exclusive(),
    )
    .with_context(|| {
        format!(
            "waiting for {} after cancelling pid {pid} failed",
            lock.display()
        )
    })?;
    let cancelled = json!({"at": now(), "holder": held, "signalled": signalled});
    if let Some(mut receipt) = ProductState::load(runtime, id, surface)? {
        receipt
            .extra
            .insert("cancelled".to_owned(), cancelled.clone());
        receipt.save(runtime)?;
    }
    file.unlock()
        .with_context(|| format!("{} cannot be released", lock.display()))?;
    Ok(json!({"product": id, "surface": surface, "cancelled": cancelled}))
}

/// Run `steps` for the installation recorded in `installed` without holding
/// `writer`, then take the lock back and record their outcome; the lock is
/// returned held. A newer installation that recorded itself in between is
/// not overwritten: the outcome is refused, naming both.
pub(super) fn after_install_unlocked(
    runtime: &Runtime,
    (id, surface, host): (&str, &str, Option<&str>),
    steps: &Value,
    installed: &mut ProductState,
    lock: &Path,
    writer: File,
) -> Result<File> {
    let recorded = serde_json::to_value(&*installed)?;
    drop(writer);
    let outcomes = super::after_install::run(id, surface, host, steps, installed);
    let writer = runtime.surface_lock(lock)?;
    let current = ProductState::load(runtime, id, surface)?;
    if serde_json::to_value(&current)? != recorded {
        bail!(
            "another installation of {id}/{surface} recorded itself while this one's after-install steps \
             ran ({}); their outcome is not written over it",
            match &outcomes {
                Ok(outcomes) => json!(outcomes).to_string(),
                Err(error) => format!("they failed: {error:#}"),
            }
        );
    }
    match outcomes {
        Ok(outcomes) => {
            installed
                .extra
                .insert("after_install".to_owned(), json!(outcomes));
            installed.save(runtime)?;
            Ok(writer)
        }
        Err(error) => {
            installed.extra.insert(
                "after_install_failure".to_owned(),
                json!(format!("{error:#}")),
            );
            installed.save(runtime)?;
            Err(error)
        }
    }
}
