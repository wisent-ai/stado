//! The `ps` row every record in this watch is built out of.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// One `ps` row, split the way `ps ax -o pid= -o ppid= -o lstart= -o command=`
/// prints it.
///
/// `lstart` is five whitespace-separated tokens (`Tue Sep  1 16:20:32 2026`)
/// and the command is everything after them, so the split is positional and
/// the command is never truncated at its first space.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcessRow {
    pub pid: String,
    pub ppid: String,
    pub started_at: String,
    pub command: String,
}

impl ProcessRow {
    /// Parse one row, or `None` when it is short enough that a field would
    /// have to be invented.
    pub fn parse(row: &str) -> Option<Self> {
        let fields: Vec<&str> = row.split_whitespace().collect();
        if fields.len() < 8 {
            return None;
        }
        Some(Self {
            pid: fields[0].to_string(),
            ppid: fields[1].to_string(),
            started_at: fields[2..7].join(" "),
            command: fields[7..].join(" "),
        })
    }

    pub fn to_json(&self) -> Value {
        json!({
            "pid": self.pid,
            "ppid": self.ppid,
            "started_at": self.started_at,
            "command": self.command,
        })
    }
}
