//! What one watch saw: the processes already there, the ones that arrived,
//! the ancestry behind each arrival, and the report that carries them.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::process_row::ProcessRow;

/// One process that matched and was already running when the watch opened.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Baseline {
    pub row: ProcessRow,
}

/// One process that matched and was NOT running when the watch opened, with
/// the ancestry read from the snapshot that first saw it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Arrival {
    pub sequence: u32,
    /// Seconds after the watch opened.
    pub after_seconds: u64,
    pub row: ProcessRow,
    /// Index 0 is the arrival itself; index 1 is its parent, and so on to
    /// pid 1 or to the first ancestor the snapshot no longer holds.
    pub ancestry: Vec<Ancestor>,
}

impl Arrival {
    /// The parent, when the snapshot still held one. `None` means the arrival
    /// was already reparented — the state that makes this question hard.
    pub fn parent(&self) -> Option<&Ancestor> {
        self.ancestry.get(1)
    }

    pub fn to_json(&self) -> Value {
        json!({
            "sequence": self.sequence,
            "after_seconds": self.after_seconds,
            "process": self.row.to_json(),
            "ancestry": self.ancestry.iter().map(Ancestor::to_json).collect::<Vec<_>>(),
        })
    }
}

/// One step up the chain from an arrival.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ancestor {
    pub depth: u32,
    /// Was this process still in the live table when the arrival was caught?
    /// A parent reported `false` had already exited, which is exactly how the
    /// child came to read `ppid 1`.
    pub alive: bool,
    pub row: ProcessRow,
}

impl Ancestor {
    pub fn to_json(&self) -> Value {
        json!({
            "depth": self.depth,
            "alive": self.alive,
            "pid": self.row.pid,
            "ppid": self.row.ppid,
            "started_at": self.row.started_at,
            "command": self.row.command,
        })
    }
}

/// Everything one watch saw.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WatchReport {
    pub host: String,
    pub matched: String,
    pub seconds: u64,
    pub interval_ms: u64,
    pub samples: u32,
    pub elapsed_seconds: u64,
    pub baseline: Vec<Baseline>,
    pub arrivals: Vec<Arrival>,
    /// Set when the host is not Darwin, naming the system it reported.
    pub unsupported: Option<String>,
}
