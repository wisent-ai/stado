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
/// on every pass -- 164 times in the log on 2026-09-05 -- and every Weles
/// trajectory that redeemed a credential read `ECONNREFUSED` at the socket,
/// including the Developer ID run that publishes desktop signing material.
/// `agent_spawned` records which processes carry the agent's own launch marker.
pub(crate) fn release_processes(install_root: &str) -> Vec<ReleaseProcess> {
    let output = match std::process::Command::new("/bin/ps")
        .args(["-eo", "pid=,pgid=,command="])
        .output()
    {
        Ok(output) => output,
        Err(_) => return Vec::new(),
    };
    let marker = format!("{}/releases/", install_root.trim_end_matches('/'));
    let mut found = Vec::new();
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        if !line.contains(&marker) {
            continue;
        }
        let mut fields = line.split_whitespace();
        let Some(pid) = fields.next().and_then(|first| first.parse::<i32>().ok()) else {
            continue;
        };
        let Some(process_group) = fields.next().and_then(|field| field.parse::<i32>().ok()) else {
            continue;
        };
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
    found
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
    let output = Command::new("/usr/sbin/lsof")
        .args([
            "-nP",
            "-Fp",
            &format!("-iTCP@127.0.0.1:{port}"),
            "-sTCP:LISTEN",
        ])
        .stderr(Stdio::null())
        .output()
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
pub(crate) fn swept_before(target: &ReleaseTargetPolicy, product: &str, pid: i32) -> bool {
    let directory = PathBuf::from(&target.state_dir).join(format!("{product}.swept"));
    if let Ok(entries) = std::fs::read_dir(&directory) {
        for entry in entries.flatten() {
            let stale = entry
                .file_name()
                .to_str()
                .and_then(|name| name.parse::<i32>().ok())
                .is_some_and(|recorded| recorded != pid && !pid_alive(recorded));
            if stale {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
    let marker = directory.join(pid.to_string());
    if marker.exists() {
        return true;
    }
    let _ = std::fs::create_dir_all(&directory);
    let _ = std::fs::write(&marker, Utc::now().to_rfc3339());
    false
}
