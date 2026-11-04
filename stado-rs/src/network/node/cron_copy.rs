//! The node its unit started takes its port back from a copy cron started.
//!
//! A host that once had no launchd domain got its node as an `@reboot` cron
//! line running `~/.stado/bin/run-com.wisent.stado-network.sh`. A later
//! install loaded the launchd unit, and an install that loads it now removes
//! that line ([`crate::deploy::local_install::activation::cron`]), but a unit
//! loaded before that removal existed kept the line: at every boot cron
//! started its copy first, the copy took the node's port, and launchd's node
//! failed to bind and was started again every ten seconds, 3130 times on one
//! Mac, while `stado network status` named the launchd pid as failed.
//!
//! So when the node cannot bind its port, and this process is the one its
//! unit started, it reads who listens there. A listener whose parent shell
//! ran the cron wrapper of this very unit is the cron copy: it is ended, the
//! cron line and wrapper are removed, and the bind is tried again once. Any
//! other listener is left alone and the bind refusal stands, naming it.

use std::time::Duration;

use crate::deploy::local_install::activation::cron::{cron_wrapper, retire_cron_fallback_of};
use crate::deploy::local_install::label;

/// The node's unit, as `stado network up` installs it.
const UNIT: &str = "stado-network";

/// How long the cron copy has to exit after SIGTERM before it is killed: a
/// node closes its listener and connections at once, so this bounds a copy
/// that does not answer the signal, not a normal exit.
const ENDS_WITHIN: Duration = Duration::from_secs(5);

/// The sentence saying what was taken back, or `None` when the port's
/// holder is no cron copy of this unit or this process is not the unit's
/// own node. `Err` when it is the cron copy and it could not be ended or its
/// cron line could not be removed.
pub(super) async fn take_back(port: u16) -> Result<Option<String>, String> {
    let label = label(UNIT);
    if crate::config_file::variable("XPC_SERVICE_NAME")?.as_deref() != Some(label.as_str()) {
        return Ok(None);
    }
    let home = crate::config_file::home()?;
    let wrapper = cron_wrapper(&home, &label);
    let Some(copy) = cron_listener(port, &wrapper.to_string_lossy())? else {
        return Ok(None);
    };
    end(copy)?;
    let runner = crate::deploy::production_runner();
    let mut said = Vec::new();
    retire_cron_fallback_of(&label, &home, &runner, &mut |line| said.push(line.to_string()))
        .await
        .map_err(|error| {
            format!(
                "the node ended the copy cron started (pid {copy}) on port {port}, but its cron line \
                 could not be removed, so the next boot starts it again: {error}"
            )
        })?;
    Ok(Some(format!(
        "port {port} was held by pid {copy}, a copy of this node cron started from {} beside the \
         unit {label}; it was ended{}",
        wrapper.display(),
        if said.is_empty() {
            String::new()
        } else {
            format!(" ({})", said.join("; "))
        }
    )))
}

/// The pid listening on `port` whose parent ran `wrapper`, if any.
fn cron_listener(port: u16, wrapper: &str) -> Result<Option<u32>, String> {
    let listed = crate::wait::output(std::process::Command::new("/usr/sbin/lsof").args([
        "-nP",
        &format!("-iTCP:{port}"),
        "-sTCP:LISTEN",
        "-Fp",
    ]))
    .map_err(|error| format!("lsof could not be run to read who listens on port {port}: {error}"))?;
    let own = std::process::id();
    for pid in String::from_utf8_lossy(&listed.stdout)
        .lines()
        .filter_map(|line| line.strip_prefix('p'))
        .filter_map(|pid| pid.parse::<u32>().ok())
        .filter(|pid| *pid != own)
    {
        let Some(parent) = parent_of(pid)? else { continue };
        let ran = match crate::deploy::service::process_arguments(parent) {
            Ok(arguments) => arguments,
            // A parent that ended between the two reads ran nothing now.
            Err(_) => continue,
        };
        if ran.iter().any(|argument| argument.contains(wrapper)) {
            return Ok(Some(pid));
        }
    }
    Ok(None)
}

/// The parent pid of `pid`, `None` when it no longer runs.
fn parent_of(pid: u32) -> Result<Option<u32>, String> {
    let read = crate::wait::output(
        std::process::Command::new("/bin/ps").args(["-o", "ppid=", "-p", &pid.to_string()]),
    )
    .map_err(|error| format!("ps could not be run to read the parent of pid {pid}: {error}"))?;
    if !read.status.success() {
        return Ok(None);
    }
    Ok(String::from_utf8_lossy(&read.stdout).trim().parse::<u32>().ok())
}

/// End the cron copy: SIGTERM, then SIGKILL if it still runs after
/// [`ENDS_WITHIN`].
fn end(pid: u32) -> Result<(), String> {
    use nix::sys::signal::{kill, Signal};
    use nix::unistd::Pid;
    let target = Pid::from_raw(
        i32::try_from(pid).map_err(|error| format!("pid {pid} is no process id: {error}"))?,
    );
    kill(target, Signal::SIGTERM)
        .map_err(|errno| format!("SIGTERM to the node copy cron started (pid {pid}) failed: {errno}"))?;
    let gone = crate::wait::blocking(
        crate::wait::Kind::Process,
        format!("the node copy cron started (pid {pid}) to exit after SIGTERM"),
        format!("pid {pid}"),
        || -> Result<bool, std::io::Error> {
            let started = std::time::Instant::now();
            while started.elapsed() < ENDS_WITHIN {
                if kill(target, None).is_err() {
                    return Ok(true);
                }
                std::thread::park_timeout(Duration::from_millis(100));
            }
            Ok(false)
        },
    )
    .map_err(|error| error.to_string())?;
    if !gone {
        kill(target, Signal::SIGKILL).map_err(|errno| {
            format!(
                "the node copy cron started (pid {pid}) did not exit within {ENDS_WITHIN:?} of SIGTERM, \
                 and SIGKILL failed: {errno}"
            )
        })?;
    }
    Ok(())
}
