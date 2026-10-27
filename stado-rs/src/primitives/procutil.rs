//! Synchronous subprocess capture, and whether a pid is a process.
//!
//! Shared by the watchdog (per-diagnostic commands) and the MCP server (CLI
//! dispatch). Reproduces the slice of Python
//! `subprocess.run(..., capture_output=True, text=True)` both consumers rely
//! on: captured stdout/stderr and the exit code. The child runs to completion.

use std::process::Command;

use nix::errno::Errno;
use nix::sys::signal::kill;
use nix::sys::wait::{waitpid, WaitPidFlag, WaitStatus};
use nix::unistd::Pid;

/// Whether `pid` is a process on this host now.
///
/// The one answer every caller gets. Separate copies disagreed: one read a
/// process owned by another account (`EPERM`) as gone, one read pid `0` or a
/// negative pid — which `kill` takes as a process group, so it answers for a
/// whole group — as alive, and only one reaped its own exited child, which
/// `kill(pid, 0)` alone reads as alive until it is reaped. Here a pid at or
/// below zero names no single process and is not one; this process's own
/// child is asked with a non-blocking `waitpid`, which reaps it when it has
/// exited; any other pid is asked with `kill(pid, 0)`, where `EPERM` means a
/// process that belongs to somebody else.
pub(crate) fn pid_alive(pid: i32) -> bool {
    if pid <= 0 {
        return false;
    }
    let process = Pid::from_raw(pid);
    match waitpid(process, Some(WaitPidFlag::WNOHANG)) {
        Ok(WaitStatus::Exited(..) | WaitStatus::Signaled(..)) => false,
        // Running, stopped or continued: a process either way.
        Ok(_) => true,
        // Not this process's child: the kernel answers for any pid.
        Err(Errno::ECHILD) => kernel_answers(process),
        Err(Errno::EINTR) => pid_alive(pid),
        Err(errno) => {
            eprintln!(
                "stado: waitpid({pid}) refused with {errno}, so whether it is a process is asked \
                 with kill(pid, 0)"
            );
            kernel_answers(process)
        }
    }
}

/// `kill(pid, 0)`: delivered, or refused only because the process belongs to
/// somebody else.
fn kernel_answers(process: Pid) -> bool {
    match kill(process, None) {
        Ok(()) => true,
        Err(errno) => errno == Errno::EPERM,
    }
}

/// Outcome of [`run_capture`]: the child exited on its own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Capture {
    pub(crate) rc: i32,
    pub(crate) stdout: String,
    pub(crate) stderr: String,
}

/// Run `argv` capturing stdout/stderr until it exits. A spawn failure
/// surfaces as the `io::Error` (Python's generic `except Exception` branch /
/// `FileNotFoundError`). A child that a signal ended has no exit code, and is
/// an error naming the signal rather than a code nobody returned.
pub(crate) fn run_capture(argv: &[String]) -> std::io::Result<Capture> {
    use std::os::unix::process::ExitStatusExt;
    let Some((program, args)) = argv.split_first() else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "empty argv",
        ));
    };
    let output = crate::wait::output(Command::new(program).args(args))?;
    let rc = match (output.status.code(), output.status.signal()) {
        (Some(code), _) => code,
        (None, Some(signal)) => {
            return Err(std::io::Error::other(format!(
                "{program} was ended by signal {signal} and returned no exit code; stderr: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            )))
        }
        (None, None) => {
            return Err(std::io::Error::other(format!(
                "{program} ended with neither an exit code nor a signal ({})",
                output.status
            )))
        }
    };
    Ok(Capture {
        rc,
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    })
}
