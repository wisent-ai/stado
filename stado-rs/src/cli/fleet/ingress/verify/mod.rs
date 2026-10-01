//! The stages, and the one thing that decides: nothing is published until the
//! address has served this build's own `/join.sh` to a request that left this
//! machine.
//!
//! Every stage that fails reports what the child itself said, read from its
//! log, rather than only the fact that the stage failed.

use std::path::Path;

pub(in crate::cli::fleet::ingress) mod children;
pub(in crate::cli::fleet::ingress) mod dns;
pub(in crate::cli::fleet::ingress) mod public;

/// The last line a child wrote to its log, for an error that has to say what
/// the child itself said.
fn log_tail(log: &Path) -> String {
    match std::fs::read_to_string(log) {
        Ok(text) => text
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .next_back()
            .unwrap_or("(the log is empty)")
            .to_string(),
        Err(exc) => format!("({} could not be read: {exc})", log.display()),
    }
}
