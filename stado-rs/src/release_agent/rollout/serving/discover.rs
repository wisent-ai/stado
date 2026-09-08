//! What the kernel and the process table say: whether a pid still lives, and
//! whether it is the exact Stado proxy that owns this product's stable bind.

use std::path::Path;
use std::process::Command;

use nix::sys::signal::{kill, Signal};
use nix::sys::wait::{waitpid, WaitPidFlag, WaitStatus};
use nix::unistd::Pid;

use crate::release_agent::state::document::proxy_state_path;
use crate::release_agent::state::records::ProcessRecord;
use crate::release_control::{BlueGreenServing, ReleaseTargetPolicy};

pub(crate) fn pid_alive(pid: i32) -> bool {
    if pid <= 1 {
        return false;
    }
    let pid = Pid::from_raw(pid);
    match waitpid(pid, Some(WaitPidFlag::WNOHANG)) {
        Ok(WaitStatus::StillAlive) => true,
        Ok(WaitStatus::Exited(..) | WaitStatus::Signaled(..)) => false,
        Ok(_) => true,
        Err(nix::errno::Errno::ECHILD) => kill(pid, None).is_ok(),
        Err(nix::errno::Errno::ESRCH) => false,
        Err(_) => kill(pid, None).is_ok(),
    }
}

pub(crate) fn terminate(record: &ProcessRecord) {
    if pid_alive(record.pid) {
        let group = Pid::from_raw(-record.pid);
        if kill(group, Signal::SIGTERM).is_err() {
            let _ = kill(Pid::from_raw(record.pid), Signal::SIGTERM);
        }
    }
}

fn process_executable_matches(pid: i32, expected: &Path) -> bool {
    #[cfg(target_os = "linux")]
    {
        let Ok(actual) = std::fs::read_link(format!("/proc/{pid}/exe")) else {
            return false;
        };
        let actual = actual.to_string_lossy();
        actual.strip_suffix(" (deleted)").unwrap_or(&actual) == expected.to_string_lossy()
    }
    #[cfg(not(target_os = "linux"))]
    {
        let Ok(output) = Command::new("/bin/ps")
            .args(["-p", &pid.to_string(), "-o", "comm="])
            .output()
        else {
            return false;
        };
        output.status.success()
            && String::from_utf8_lossy(&output.stdout).trim() == expected.to_string_lossy()
    }
}

pub(crate) fn proxy_process_matches(
    pid: i32,
    target: &ReleaseTargetPolicy,
    serving: &BlueGreenServing,
    product: &str,
) -> Result<bool, String> {
    if !pid_alive(pid) {
        return Ok(false);
    }
    let executable = std::env::current_exe()
        .map_err(|error| format!("cannot resolve Stado executable: {error}"))?;
    if !process_executable_matches(pid, &executable) {
        return Ok(false);
    }
    let expected_command = format!(
        "{} release proxy --state {} --bind {}",
        executable.display(),
        proxy_state_path(target, product).display(),
        serving.stable_bind
    );
    let output = Command::new("/bin/ps")
        .args(["-ww", "-p", &pid.to_string(), "-o", "command="])
        .output()
        .map_err(|error| format!("cannot inspect stable proxy pid {pid}: {error}"))?;
    Ok(output.status.success()
        && String::from_utf8_lossy(&output.stdout).trim() == expected_command)
}

/// Find the live Stado proxy whose executable and complete argument vector own
/// this exact state file and bind.
///
/// A release proxy binds before entering its accept loop and exits when the
/// bind fails. After the caller's spawn grace period, a live exact match is
/// therefore the process that owns the bind, not merely another program that
/// happens to answer the product's readiness URL.
pub(crate) fn exact_proxy_pid(
    target: &ReleaseTargetPolicy,
    serving: &BlueGreenServing,
    product: &str,
) -> Result<Option<i32>, String> {
    let executable = std::env::current_exe()
        .map_err(|error| format!("cannot resolve Stado executable: {error}"))?;
    let expected_command = format!(
        "{} release proxy --state {} --bind {}",
        executable.display(),
        proxy_state_path(target, product).display(),
        serving.stable_bind
    );
    let output = Command::new("/bin/ps")
        .args(["axww", "-o", "pid=", "-o", "command="])
        .output()
        .map_err(|error| format!("cannot inspect stable proxy processes: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "cannot inspect stable proxy processes: /bin/ps exited {}",
            output.status
        ));
    }
    let mut matches = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| {
            let line = line.trim_start();
            let split = line.find(char::is_whitespace)?;
            let pid = line[..split].parse::<i32>().ok()?;
            (line[split..].trim_start() == expected_command
                && pid_alive(pid)
                && process_executable_matches(pid, &executable))
            .then_some(pid)
        })
        .collect::<Vec<_>>();
    matches.sort_unstable();
    matches.dedup();
    match matches.as_slice() {
        [pid] => Ok(Some(*pid)),
        [] => Ok(None),
        many => Err(format!(
            "{} live Stado release proxies claim {} with state {}",
            many.len(),
            serving.stable_bind,
            proxy_state_path(target, product).display()
        )),
    }
}
