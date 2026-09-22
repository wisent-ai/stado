//! What the kernel says about one process: whether a pid still lives, how it
//! is asked to stop, and whether it runs the executable it should.
//!
//! Split out of `serving/discover.rs`, which had grown past the module line
//! cap; recognising the release proxy among those processes stays there.

use std::io::Read;
use std::path::Path;
use std::process::Command;

use nix::sys::signal::{kill, Signal};
use nix::sys::wait::{waitpid, WaitPidFlag, WaitStatus};
use nix::unistd::Pid;

use crate::release_agent::state::records::ProcessRecord;

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

/// The control peer is the actual host executable, not merely a process that
/// supplied its PID in JSON. Managed and global installations can be separate
/// byte-identical files; the ordinary same-file predicate remains unchanged.
pub(crate) fn controller_process_matches(pid: i32) -> Result<bool, String> {
    if !pid_alive(pid) {
        return Ok(false);
    }
    let expected = std::env::current_exe()
        .map_err(|error| format!("cannot resolve Stado executable: {error}"))?;
    let output = Command::new("/bin/ps")
        .args(["-ww", "-p", &pid.to_string(), "-o", "command="])
        .output()
        .map_err(|error| format!("cannot inspect proxy owner pid {pid}: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "cannot inspect proxy owner pid {pid}: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    let observed = String::from_utf8_lossy(&output.stdout);
    let command = observed.trim();
    for (index, _) in command.match_indices(" serve") {
        let remaining = &command[index + " serve".len()..];
        if !remaining.is_empty() && !remaining.starts_with(char::is_whitespace) {
            continue;
        }
        let program = Path::new(&command[..index]);
        if process_executable_matches(pid, program)
            && (same_executable(program, &expected)
                || same_native_image(program, &expected).map_err(|error| {
                    format!(
                        "cannot compare proxy owner image {} with {}: {error}",
                        program.display(),
                        expected.display()
                    )
                })?)
        {
            return Ok(true);
        }
    }
    Ok(false)
}

fn same_native_image(left: &Path, right: &Path) -> std::io::Result<bool> {
    let mut left = std::fs::File::open(left)?;
    let mut right = std::fs::File::open(right)?;
    let left_metadata = left.metadata()?;
    let right_metadata = right.metadata()?;
    if !left_metadata.is_file()
        || !right_metadata.is_file()
        || left_metadata.len() != right_metadata.len()
    {
        return Ok(false);
    }
    let mut remaining = left_metadata.len();
    let mut left_bytes = [0_u8; 8192];
    let mut right_bytes = [0_u8; 8192];
    while remaining != 0 {
        let count = remaining.min(left_bytes.len() as u64) as usize;
        left.read_exact(&mut left_bytes[..count])?;
        right.read_exact(&mut right_bytes[..count])?;
        if left_bytes[..count] != right_bytes[..count] {
            return Ok(false);
        }
        remaining -= count as u64;
    }
    Ok(true)
}
