//! Synchronous subprocess capture.
//!
//! Shared by the watchdog (per-diagnostic commands) and the MCP server (CLI
//! dispatch). Reproduces the slice of Python
//! `subprocess.run(..., capture_output=True, text=True)` both consumers rely
//! on: captured stdout/stderr and the exit code. The child runs to completion.

use std::process::Command;

/// Outcome of [`run_capture`]: the child exited on its own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Capture {
    pub(crate) rc: i32,
    pub(crate) stdout: String,
    pub(crate) stderr: String,
}

/// Run `argv` capturing stdout/stderr until it exits. A spawn failure
/// surfaces as the `io::Error` (Python's generic `except Exception` branch /
/// `FileNotFoundError`).
pub(crate) fn run_capture(argv: &[String]) -> std::io::Result<Capture> {
    let Some((program, args)) = argv.split_first() else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "empty argv",
        ));
    };
    let output = Command::new(program).args(args).output()?;
    Ok(Capture {
        rc: output.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    })
}
