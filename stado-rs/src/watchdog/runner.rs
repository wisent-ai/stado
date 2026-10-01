//! The fault-isolation seam: one diagnostic command's outcome, the runner
//! trait tests inject a fake for, and the production subprocess runner.

use crate::primitives::procutil::{run_capture, Capture};

/// Outcome of one diagnostic command that ran to completion (Python
/// `subprocess.run` result).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunOutcome {
    pub rc: i32,
    pub stdout: String,
    pub stderr: String,
}

/// Fault-isolation seam: how a diagnostic command is executed. Tests inject
/// a fake; production uses [`SystemRunner`].
pub trait CommandRunner: Send + Sync {
    /// Run `argv` capturing output until it exits. `Err` = the process could
    /// not be spawned at all (Python's generic `except Exception` branch,
    /// e.g. `FileNotFoundError` when the binary is not installed on the box).
    fn run(&self, argv: &[String]) -> std::io::Result<RunOutcome>;
}

/// Production runner: real subprocesses via [`run_capture`].
pub struct SystemRunner;

impl CommandRunner for SystemRunner {
    fn run(&self, argv: &[String]) -> std::io::Result<RunOutcome> {
        let Capture { rc, stdout, stderr } = run_capture(argv)?;
        Ok(RunOutcome { rc, stdout, stderr })
    }
}
