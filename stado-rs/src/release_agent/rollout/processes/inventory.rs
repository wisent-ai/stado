//! Every process running out of one product's releases directory, what the
//! kernel says is listening, and which pids this agent has already swept.

use std::path::PathBuf;
use std::process::{Command, Stdio};

use chrono::Utc;

use crate::release_agent::rollout::serving::discover::pid_alive;
use crate::release_control::ReleaseTargetPolicy;

#[derive(Debug)]
pub(crate) struct ReleaseProcess {
    pub(crate) pid: i32,
    pub(crate) process_group: i32,
    pub(crate) version: String,
    pub(crate) port: Option<u16>,
    pub(crate) release_dir: PathBuf,
    /// Whether this process was started by `spawn_release`: that path runs the
    /// launcher through `env STADO_RELEASE_PRODUCT=<product> ...`, so the marker
    /// sits in the argument vector of the group leader and of every child that
    /// re-executes the same command line. A process running the release
    /// binary without it was started by someone else.
    pub(crate) agent_spawned: bool,
}

/// Every process running out of this product's `releases/` directory, including
/// the immutable release directory named by its exact argument vector.
///
/// Not every one of them is this agent's. `release_processes` used to say
/// "nothing else executes from that directory", and that was false on the
/// always-on Mac: the Weles worker starts `skarbiec capability-serve` out of
/// the attested Skarbiec release directory on purpose, because a broker from
/// any other path could belong to a different Skarbiec generation. The state
/// file never names that process, so `sweep_leaked_processes` sent it SIGTERM
/// on every pass, and every Weles trajectory that redeemed a credential
/// read `ECONNREFUSED` at the socket,
/// including the Developer ID run that publishes desktop signing material.
/// `agent_spawned` records which processes carry the agent's own launch marker.
///
/// A process table that cannot be read is the error, not an empty table:
/// read as empty, reconcile retired a serving release proxy as orphaned.
pub(crate) fn release_processes(install_root: &str) -> Result<Vec<ReleaseProcess>, String> {
    let output = crate::wait::output(
        std::process::Command::new("/bin/ps").args(["-eo", "pid=,pgid=,command="]),
    )
    .map_err(|error| format!("cannot read the process table: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "cannot read the process table: /bin/ps exited {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let marker = format!("{}/releases/", install_root.trim_end_matches('/'));
    let mut found = Vec::new();
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        if !line.contains(&marker) {
            continue;
        }
        let mut fields = line.split_whitespace();
        // A release process whose ids do not read is refused, never skipped: a
        // skipped one is a running release the rollout does not know about.
        let number = |field: Option<&str>, what: &str| -> Result<i32, String> {
            let text =
                field.ok_or_else(|| format!("the process table line {line:?} has no {what}"))?;
            text.parse::<i32>().map_err(|error| {
                format!(
                    "the process table line {line:?} has an unreadable {what} {text:?}: {error}"
                )
            })
        };
        let pid = number(fields.next(), "process id")?;
        let process_group = number(fields.next(), "process group")?;
        let arguments: Vec<&str> = fields.collect();
        let Some((version, release_dir)) = arguments.iter().find_map(|argument| {
            let path = argument
                .split_once('=')
                .map_or(*argument, |(_, value)| value);
            let (prefix, tail) = path.split_once(&marker)?;
            let mut components = tail.split('/');
            let version = components.next()?;
            let platform = components.next()?;
            if version.is_empty() || platform.is_empty() {
                return None;
            }
            Some((
                version.to_string(),
                PathBuf::from(format!("{prefix}{marker}{version}/{platform}")),
            ))
        }) else {
            continue;
        };
        let mut port = None;
        for pair in arguments.windows(2) {
            if pair[0] == "--port" {
                port = pair[1].parse().ok();
            }
        }
        let agent_spawned = arguments
            .iter()
            .any(|argument| argument.starts_with("STADO_RELEASE_PRODUCT="));
        found.push(ReleaseProcess {
            pid,
            process_group,
            version,
            port,
            release_dir,
            agent_spawned,
        });
    }
    Ok(found)
}

/// SIGTERM to one release process. One already gone counts as ended; any other
/// failure is answered with the version, pid and cause, so a caller never
/// reports as ended a process the signal did not reach.
pub(crate) fn terminate(process: &ReleaseProcess) -> Result<(), String> {
    let pid = nix::unistd::Pid::from_raw(process.pid);
    match nix::sys::signal::kill(pid, nix::sys::signal::Signal::SIGTERM) {
        Ok(()) | Err(nix::errno::Errno::ESRCH) => Ok(()),
        Err(errno) => Err(format!(
            "SIGTERM to {} pid {} failed: {errno}",
            process.version, process.pid
        )),
    }
}

/// The pid the kernel says is listening on a loopback port, or `None` when
/// nothing is or the socket table could not be read.
///
/// `ReleaseProcess::port` is what a launcher was told; this is what a process
/// does. They differ for any release whose argument vector does not carry
/// `--port` -- skarbiec's does not -- and for such a release the argv answer
/// is always `None`, which is how the sweep came to kill the one process
/// serving the proxy's upstream while believing it carried no traffic.
pub(crate) fn listener_pid(port: Option<u16>) -> Option<i32> {
    let port = port?;
    let output = crate::wait::output(
        Command::new("/usr/sbin/lsof")
            .args([
                "-nP",
                "-Fp",
                &format!("-iTCP@127.0.0.1:{port}"),
                "-sTCP:LISTEN",
            ])
            .stderr(Stdio::null()),
    )
    .ok()?;
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .find_map(|line| line.strip_prefix('p')?.parse().ok())
}

/// Whether this pid was already swept on an earlier pass, recording it if not.
///
/// The record is one file per pid under the product's state directory, so it
/// survives the agent process and never outlives the pid's next reuse by more
/// than the file's own name: `pid_alive` is checked by the caller before this
/// is consulted, and a pid that has exited takes its marker with it on the next
/// pass that finds the file without the process.
///
/// A record that cannot be read, cleared or written is an error naming the
/// path: an unwritten marker kept a process that ignores SIGTERM on SIGTERM
/// forever, and a stale marker left behind would SIGKILL the next process
/// that reuses its pid on first sight.
pub(crate) fn swept_before(
    target: &ReleaseTargetPolicy,
    product: &str,
    pid: i32,
) -> Result<bool, String> {
    let directory = PathBuf::from(&target.state_dir).join(format!("{product}.swept"));
    match std::fs::read_dir(&directory) {
        Ok(entries) => {
            for entry in entries {
                let entry = entry
                    .map_err(|error| format!("cannot list {}: {error}", directory.display()))?;
                let stale = entry
                    .file_name()
                    .to_str()
                    .and_then(|name| name.parse::<i32>().ok())
                    .is_some_and(|recorded| recorded != pid && !pid_alive(recorded));
                if stale {
                    std::fs::remove_file(entry.path()).map_err(|error| {
                        format!(
                            "cannot clear the stale sweep marker {}: {error}",
                            entry.path().display()
                        )
                    })?;
                }
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(format!("cannot list {}: {error}", directory.display())),
    }
    let marker = directory.join(pid.to_string());
    match std::fs::symlink_metadata(&marker) {
        Ok(_) => return Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(format!("cannot read {}: {error}", marker.display())),
    }
    std::fs::create_dir_all(&directory)
        .map_err(|error| format!("cannot create {}: {error}", directory.display()))?;
    std::fs::write(&marker, Utc::now().to_rfc3339())
        .map_err(|error| format!("cannot write {}: {error}", marker.display()))?;
    Ok(false)
}
