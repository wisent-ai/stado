//! A child that ran and exited unsuccessfully, with its exit code kept so a
//! caller can tell what kind of failure it was: a Stado child that could not
//! reach what it needs exits with the fleet's retry exit code, and the caller
//! classifies the failure by that code, never by the child's words.

use std::process::Output;

#[derive(Debug)]
pub struct CommandFailed {
    pub code: Option<i32>,
    message: String,
}

impl CommandFailed {
    /// A child whose failure is said as `message`, its text being what
    /// `checked` always reported.
    pub(crate) fn new(code: Option<i32>, message: String) -> Self {
        Self { code, message }
    }

    /// `what` exited unsuccessfully with `output`: its status and its
    /// standard error, trimmed.
    pub(crate) fn exited(what: &str, output: &Output) -> Self {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Self::new(
            output.status.code(),
            format!("{what} exited {}: {}", output.status, stderr.trim()),
        )
    }
}

impl std::fmt::Display for CommandFailed {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for CommandFailed {}
