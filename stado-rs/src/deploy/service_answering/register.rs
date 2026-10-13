//! The host's register of service recoveries: one JSON line per service
//! the host found standing and acted on, appended under the Stado home, so
//! `stado service recoveries` and Stado Desktop can read what was done, to
//! which service, with which requests held, and what the repair and the
//! restart answered. A register that cannot be read is said with its path
//! and cause; one that is not there holds no recovery.

use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::cli::CmdError;

/// Where the register lives under the Stado home.
pub const REGISTER_FILE: &str = "service-recoveries.jsonl";

/// One recovery as it was recorded.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Recovery {
    /// When the recovery was recorded, after the restart.
    pub at: String,
    pub host: String,
    pub service: String,
    pub unit: String,
    /// The declared question that stood: method and path.
    pub question: String,
    /// Since when the question stood, and for how long, against the bound.
    pub stood_since: String,
    pub stood_seconds: u64,
    pub bound_seconds: u64,
    /// The requests this host's resolver held unanswered to the service at
    /// the time, as `resolver-waiting.json` listed them; empty when the
    /// resolver listed none or publishes no list.
    pub held_requests: Vec<Value>,
    /// What the declared repair step answered; `null` when none is declared.
    pub repair: Value,
    /// What the restart answered, or the refusal it ended with.
    pub restart: Value,
    /// Whether the restart ended well.
    pub restarted: bool,
    /// What the alert delivery answered.
    pub alert: Value,
}

pub fn path(home: &Path) -> PathBuf {
    home.join(".stado").join(REGISTER_FILE)
}

/// Append `recovery` to the register.
pub fn append(home: &Path, recovery: &Recovery) -> Result<(), CmdError> {
    let path = path(home);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| {
            CmdError::click(format!("cannot make {}: {error}", parent.display()))
        })?;
    }
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|error| {
            CmdError::click(format!(
                "cannot open the recovery register {}: {error}",
                path.display()
            ))
        })?;
    let line = serde_json::to_string(recovery)?;
    writeln!(file, "{line}").map_err(|error| {
        CmdError::click(format!(
            "cannot append to the recovery register {}: {error}",
            path.display()
        ))
    })
}

/// Every recovery the register holds, oldest first; none when the register
/// is not there. A line that is not a recovery is refused with the line
/// itself, since a register nobody can read whole says nothing certain.
pub fn read(home: &Path) -> Result<Vec<Recovery>, CmdError> {
    let path = path(home);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(CmdError::click(format!(
                "the recovery register {} cannot be read: {error}",
                path.display()
            )))
        }
    };
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            serde_json::from_str(line).map_err(|error| {
                CmdError::click(format!(
                    "a line of the recovery register {} is not a recovery: {error}: {line}",
                    path.display()
                ))
            })
        })
        .collect()
}
