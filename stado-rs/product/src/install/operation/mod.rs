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

/// The after-install run the surface's receipt records whose process the
/// process table still names with its recorded birth time; `None` when there
/// is none, or when the recorded process has ended (the run then never
/// recorded an outcome, and the installation that holds the lock next goes
/// on over it as it does without `--wait`).
fn running_after_install(runtime: &Runtime, id: &str, surface: &str) -> Result<Option<Value>> {
    Ok(post_placement(runtime, id, surface)?.filter(|operation| operation["running"] == true))
}

/// Wait, as a `czekam:` process wait `stado waits` lists, until the process
/// `running` records (pid and birth time) no longer runs: an older
/// installation of `id`/`surface` finishing its after-install steps without
/// the lock. Its birth time is read again on every wake, so a pid handed to
/// another process ends the wait as the end of the recorded one does.
fn wait_for_after_install(id: &str, surface: &str, running: &Value) -> Result<()> {
    let pid = i32::try_from(
        running["pid"]
            .as_i64()
            .context("after-install operation names no pid")?,
    )?;
    let born = running["started_at"]
        .as_str()
        .context("after-install operation records no process birth time")?
        .to_owned();
    eprintln!(
        "waiting for the installation of {id}/{surface} by pid {pid} to finish its after-install \
         steps {}",
        running["steps"]
    );
    stado_wait::blocking(
        stado_wait::Kind::Process,
        format!("pid {pid} finishing the after-install steps of {id}/{surface}"),
        format!("pid {pid}"),
        || -> std::result::Result<(), std::io::Error> {
            while started_at(pid).map_err(std::io::Error::other)?.as_deref() == Some(born.as_str()) {
                process_exit(pid)?;
            }
            Ok(())
        },
    )
    .with_context(|| format!("waiting for pid {pid}, the installation of {id}/{surface}"))
}

/// Block until `pid` exits, as the kernel reports it (`kqueue` `NOTE_EXIT`):
/// no polling interval stands between its end and this return. A pid that
/// has already exited returns at once.
#[cfg(target_os = "macos")]
fn process_exit(pid: i32) -> std::io::Result<()> {
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    // SAFETY: kqueue takes no arguments and returns a new descriptor or -1.
    let descriptor = unsafe { libc::kqueue() };
    if descriptor.is_negative() {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: the descriptor was just created and is owned here alone.
    let queue = unsafe { OwnedFd::from_raw_fd(descriptor) };
    let change = libc::kevent {
        ident: pid as libc::uintptr_t,
        filter: libc::EVFILT_PROC,
        flags: libc::EV_ADD | libc::EV_ENABLE | libc::EV_ONESHOT,
        fflags: libc::NOTE_EXIT,
        data: Default::default(),
        udata: std::ptr::null_mut(),
    };
    let mut observed = [change];
    loop {
        // SAFETY: one change and one event slot, both live for the call.
        let result = unsafe {
            libc::kevent(
                queue.as_raw_fd(),
                &change,
                1,
                observed.as_mut_ptr(),
                1,
                std::ptr::null(),
            )
        };
        if result >= 0 {
            return Ok(());
        }
        let cause = std::io::Error::last_os_error();
        match cause.raw_os_error() {
            // The process ended before it could be watched.
            Some(libc::ESRCH) => return Ok(()),
            Some(libc::EINTR) => continue,
            _ => return Err(cause),
        }
    }
}

/// Block until `pid` exits, as the kernel reports it (`pidfd_open` becomes
/// readable when the process ends). A pid that has already exited returns at
/// once.
#[cfg(target_os = "linux")]
fn process_exit(pid: i32) -> std::io::Result<()> {
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    // SAFETY: pidfd_open takes a pid and flags and returns a descriptor or -1.
    let descriptor = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) };
    if descriptor < 0 {
        let cause = std::io::Error::last_os_error();
        return match cause.raw_os_error() {
            Some(libc::ESRCH) => Ok(()),
            _ => Err(cause),
        };
    }
    // SAFETY: the descriptor was just created and is owned here alone.
    let pidfd = unsafe { OwnedFd::from_raw_fd(i32::try_from(descriptor).map_err(std::io::Error::other)?) };
    let mut poll = libc::pollfd {
        fd: pidfd.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    loop {
        // SAFETY: one pollfd that lives for the call; -1 waits without a deadline.
        let result = unsafe { libc::poll(&mut poll, 1, -1) };
        if result >= 0 {
            return Ok(());
        }
        let cause = std::io::Error::last_os_error();
        if cause.raw_os_error() != Some(libc::EINTR) {
            return Err(cause);
        }
    }
}

/// No kernel report of a process's end is read on this platform, so an
/// installation that would wait for another's after-install steps is
/// refused, naming the process, instead of polling it.
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn process_exit(pid: i32) -> std::io::Result<()> {
    Err(std::io::Error::other(format!(
        "this platform reports no process's end to Stado, so pid {pid} cannot be waited for; \
         install again once it has finished"
    )))
}
