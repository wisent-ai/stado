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

/// Two paths that name one executable, whatever spelling each was reached by.
///
/// `~/.local/bin/stado` is a symlink to `~/.stado/bin/stado` on every Mac in
/// this fleet, so a process started through the first and a check that
/// resolved the second are the same program under two names. String equality
/// called them different, and the credential plane refused every read with
/// `recorded stable proxy pid N does not match the exact executable and
/// arguments` while the proxy it was refusing was its own.
pub(crate) fn same_executable(left: &Path, right: &Path) -> bool {
    if left == right {
        return true;
    }
    match (left.canonicalize(), right.canonicalize()) {
        (Ok(left), Ok(right)) => left == right,
        _ => false,
    }
}

/// Does `pid` run the executable at `expected`?
///
/// The two systems answer this in different words. Linux hands the resolved
/// image through `/proc/<pid>/exe`. macOS answers `ps -o comm=`, and on this
/// fleet that prints the bare program name — `stado`, not a path — so the
/// former comparison against a full path was false for every process on
/// every Mac. The full path is still proved, by the complete argument vector
/// the callers compare; this check is the image, and a bare name is compared
/// as one.
pub(crate) fn process_executable_matches(pid: i32, expected: &Path) -> bool {
    #[cfg(target_os = "linux")]
    {
        let Ok(actual) = std::fs::read_link(format!("/proc/{pid}/exe")) else {
            return false;
        };
        let actual = actual.to_string_lossy();
        let actual = actual.strip_suffix(" (deleted)").unwrap_or(&actual);
        same_executable(Path::new(actual), expected)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let Ok(output) = Command::new("/bin/ps")
            .args(["-p", &pid.to_string(), "-o", "comm="])
            .output()
        else {
            return false;
        };
        if !output.status.success() {
            return false;
        }
        let actual = String::from_utf8_lossy(&output.stdout);
        let actual = Path::new(actual.trim());
        if actual.as_os_str().is_empty() {
            return false;
        }
        if actual.is_absolute() {
            return same_executable(actual, expected);
        }
        actual.file_name() == expected.file_name()
    }
}

/// The argument vector one Stado release proxy is recognised by, without its
/// program name: the program is compared as an executable, not as text.
fn proxy_arguments(
    target: &ReleaseTargetPolicy,
    serving: &BlueGreenServing,
    product: &str,
) -> String {
    format!(
        "release proxy --state {} --bind {}",
        proxy_state_path(target, product).display(),
        serving.stable_bind
    )
}

/// Split one `ps -o command=` line into the program it ran and the rest.
fn program_and_arguments(line: &str) -> Option<(&str, &str)> {
    let line = line.trim();
    let split = line.find(char::is_whitespace)?;
    Some((&line[..split], line[split..].trim_start()))
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
    let expected_arguments = proxy_arguments(target, serving, product);
    let output = Command::new("/bin/ps")
        .args(["-ww", "-p", &pid.to_string(), "-o", "command="])
        .output()
        .map_err(|error| format!("cannot inspect stable proxy pid {pid}: {error}"))?;
    if !output.status.success() {
        return Ok(false);
    }
    let observed = String::from_utf8_lossy(&output.stdout);
    Ok(
        program_and_arguments(&observed).is_some_and(|(program, arguments)| {
            arguments == expected_arguments && same_executable(Path::new(program), &executable)
        }),
    )
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
    let expected_arguments = proxy_arguments(target, serving, product);
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
            let (program, arguments) = program_and_arguments(&line[split..])?;
            (arguments == expected_arguments
                && same_executable(Path::new(program), &executable)
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

/// The `lsof` this host carries, or `None` when it carries none.
///
/// Both paths are real: macOS ships `/usr/sbin/lsof`, most Linux
/// distributions `/usr/bin/lsof`, and the host programs in
/// [`crate::deploy`] already probe exactly this pair in this order.
pub(crate) fn lsof_binary() -> Option<&'static Path> {
    ["/usr/sbin/lsof", "/usr/bin/lsof"]
        .into_iter()
        .map(Path::new)
        .find(|candidate| candidate.is_file())
}

/// Who holds the stable bind right now, when that holder is not this
/// product's own release proxy.
///
/// `Ok(None)` is both "nothing is listening" and "this host cannot tell":
/// a missing or failing `lsof` is an unknown, and an unknown never refuses a
/// rollout. Only a listener that is positively not the proxy answers `Some`,
/// and the sentence names the pid and the command, because the fact an
/// operator needs is which program to move.
///
/// It exists because a candidate that cannot bind dies with
/// `Address already in use (os error 48)` inside a stderr tail, ninety
/// seconds after it was spawned, and the register filed that as
/// `unclassified`. On `lukasz-macbook` on 2026-09-20 the holder of Skarbiec's
/// `127.0.0.1:18787` was this same product: the resolver's `weles-admission`
/// adapter for consumer `skarbiec-weles-credential-client`, declared on the
/// port the release policy declares as the stable bind.
pub(crate) fn foreign_stable_bind_holder(
    target: &ReleaseTargetPolicy,
    serving: &BlueGreenServing,
    product: &str,
) -> Result<Option<String>, String> {
    let port = serving
        .stable_bind
        .rsplit(':')
        .next()
        .filter(|port| port.chars().all(|character| character.is_ascii_digit()))
        .ok_or_else(|| {
            format!(
                "{product} stable bind {} names no port",
                serving.stable_bind
            )
        })?;
    let Some(lsof) = lsof_binary() else {
        return Ok(None);
    };
    let Ok(output) = Command::new(lsof)
        .args(["-nP", &format!("-iTCP:{port}"), "-sTCP:LISTEN", "-Fpc"])
        .output()
    else {
        return Ok(None);
    };
    let ours = exact_proxy_pid(target, serving, product)?;
    let mut holder: Option<(i32, String)> = None;
    for field in String::from_utf8_lossy(&output.stdout).lines() {
        match field.split_at(1) {
            ("p", pid) => {
                let pid = pid.trim().parse::<i32>().unwrap_or_default();
                holder = (pid > 0 && Some(pid) != ours).then_some((pid, String::new()));
            }
            ("c", command) => {
                if let Some((_, name)) = holder.as_mut() {
                    *name = command.trim().to_string();
                }
            }
            _ => {}
        }
        if let Some((pid, name)) = holder.as_ref().filter(|(_, name)| !name.is_empty()) {
            return Ok(Some(format!(
                "{} is held by pid {pid} ({name}), which is not {product}'s release proxy",
                serving.stable_bind
            )));
        }
    }
    Ok(holder.map(|(pid, _)| {
        format!(
            "{} is held by pid {pid}, which is not {product}'s release proxy",
            serving.stable_bind
        )
    }))
}
