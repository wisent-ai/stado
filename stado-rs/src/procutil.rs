//! Synchronous subprocess capture with a timeout.
//!
//! Shared by the watchdog (per-diagnostic command timeout) and the MCP
//! server (600 s CLI dispatch timeout). Reproduces the slice of Python
//! `subprocess.run(..., capture_output=True, text=True, timeout=...)` both
//! consumers rely on: captured stdout/stderr, the exit code, and a kill +
//! partial-output result on timeout (`subprocess.TimeoutExpired`).

use std::io::Read;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

/// Outcome of [`run_capture`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Capture {
    /// The child exited on its own.
    Completed { rc: i32, stdout: String, stderr: String },
    /// The deadline passed; the child was killed and reaped. stdout/stderr
    /// hold whatever the child wrote before the kill (Python
    /// `TimeoutExpired.stdout` / `.stderr`).
    TimedOut { stdout: String, stderr: String },
}

/// Run `argv` capturing stdout/stderr, killing the child after `timeout`.
/// A spawn failure surfaces as the `io::Error` (Python's generic
/// `except Exception` branch / `FileNotFoundError`).
pub(crate) fn run_capture(argv: &[String], timeout: Duration) -> std::io::Result<Capture> {
    let Some(program) = argv.first() else {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "empty argv"));
    };
    let mut child = Command::new(program)
        .args(&argv[1..])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let mut out_pipe = child.stdout.take().expect("stdout piped");
    let mut err_pipe = child.stderr.take().expect("stderr piped");
    let out_thread =
        thread::spawn(move || -> Vec<u8> { let mut buf = Vec::new(); out_pipe.read_to_end(&mut buf).ok(); buf });
    let err_thread =
        thread::spawn(move || -> Vec<u8> { let mut buf = Vec::new(); err_pipe.read_to_end(&mut buf).ok(); buf });
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait()? {
            Some(status) => {
                let stdout = out_thread.join().unwrap_or_default();
                let stderr = err_thread.join().unwrap_or_default();
                return Ok(Capture::Completed {
                    rc: status.code().unwrap_or(-1),
                    stdout: String::from_utf8_lossy(&stdout).into_owned(),
                    stderr: String::from_utf8_lossy(&stderr).into_owned(),
                });
            }
            None if Instant::now() >= deadline => {
                child.kill().ok();
                child.wait().ok();
                let stdout = out_thread.join().unwrap_or_default();
                let stderr = err_thread.join().unwrap_or_default();
                return Ok(Capture::TimedOut {
                    stdout: String::from_utf8_lossy(&stdout).into_owned(),
                    stderr: String::from_utf8_lossy(&stderr).into_owned(),
                });
            }
            None => thread::sleep(Duration::from_millis(20)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn captures_output_and_rc() {
        let cap = run_capture(
            &argv(&["sh", "-c", "echo out; echo err >&2; exit 3"]),
            Duration::from_secs(10),
        )
        .unwrap();
        assert_eq!(
            cap,
            Capture::Completed { rc: 3, stdout: "out\n".into(), stderr: "err\n".into() }
        );
    }

    #[test]
    fn kills_after_timeout_with_partial_output() {
        let cap = run_capture(
            &argv(&["sh", "-c", "echo before; sleep 30"]),
            Duration::from_millis(300),
        )
        .unwrap();
        assert_eq!(cap, Capture::TimedOut { stdout: "before\n".into(), stderr: String::new() });
    }

    #[test]
    fn missing_program_is_an_io_error() {
        assert!(run_capture(&argv(&["definitely-not-a-real-binary-xyz"]), Duration::from_secs(1)).is_err());
    }
}
