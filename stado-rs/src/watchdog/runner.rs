//! The fault-isolation seam: one diagnostic command's outcome, the runner
//! trait tests inject a fake for, and the production subprocess runner.

use std::time::Duration;

use crate::procutil::{run_capture, Capture};

/// Outcome of one diagnostic command (Python `subprocess.run` result or a
/// `TimeoutExpired` after the child was killed).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunOutcome {
    Completed {
        rc: i32,
        stdout: String,
        stderr: String,
    },
    TimedOut {
        stdout: String,
        stderr: String,
    },
}

/// Fault-isolation seam: how a diagnostic command is executed. Tests inject
/// a fake; production uses [`SystemRunner`].
pub trait CommandRunner: Send + Sync {
    /// Run `argv` capturing output with a `timeout_s` kill deadline.
    /// `Err` = the process could not be spawned at all (Python's generic
    /// `except Exception` branch, e.g. `FileNotFoundError` when the binary
    /// is not installed on the box).
    fn run(&self, argv: &[String], timeout_s: u64) -> std::io::Result<RunOutcome>;
}

/// Production runner: real subprocesses via [`run_capture`].
pub struct SystemRunner;

impl CommandRunner for SystemRunner {
    fn run(&self, argv: &[String], timeout_s: u64) -> std::io::Result<RunOutcome> {
        Ok(match run_capture(argv, Duration::from_secs(timeout_s))? {
            Capture::Completed { rc, stdout, stderr } => {
                RunOutcome::Completed { rc, stdout, stderr }
            }
            Capture::TimedOut { stdout, stderr } => RunOutcome::TimedOut { stdout, stderr },
        })
    }
}
