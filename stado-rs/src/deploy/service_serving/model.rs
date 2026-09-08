//! The document model: one process holding a declared port, one declared port
//! and who answers on it, everything the remote program reported, and one
//! port's verdict once this side is done with it.

use serde::{Deserialize, Serialize};

use super::OWNER_RESOLVED;

/// One process holding a declared port.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Holder {
    pub pid: String,
    /// The executable, as `ps -o comm=` prints it. Never the argument vector:
    /// a command line can carry a secret and this report does not need one.
    pub comm: String,
    /// The launchd label whose job this pid belongs to, empty when unresolved.
    pub owner: String,
    /// [`OWNER_RESOLVED`](super::OWNER_RESOLVED) or [`OWNER_UNKNOWN`](super::OWNER_UNKNOWN).
    pub owner_state: String,
}

/// One declared port and who answers on it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PortReport {
    pub port: u16,
    pub holders: Vec<Holder>,
}

/// Everything the remote script reported.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServingReport {
    pub unit: String,
    pub unit_path: String,
    /// `yes` when launchd knows the label in the readable domain.
    pub loaded: String,
    /// The pid launchd reports for the label, empty when it holds none.
    pub launchd_pid: String,
    /// [`LISTENERS_READ`](super::LISTENERS_READ) or [`LISTENERS_FAILED`](super::LISTENERS_FAILED).
    pub listeners_state: String,
    pub ports: Vec<PortReport>,
}

/// One port's verdict, and the sentence an operator acts on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortVerdict {
    pub port: u16,
    pub verdict: &'static str,
    pub holders: Vec<Holder>,
}

impl PortVerdict {
    /// Every holder that is not this unit, spelled for a table cell.
    pub fn holder_cell(&self) -> String {
        if self.holders.is_empty() {
            return "-".to_string();
        }
        self.holders
            .iter()
            .map(|holder| {
                let owner = if holder.owner_state == OWNER_RESOLVED && !holder.owner.is_empty() {
                    holder.owner.clone()
                } else {
                    "owner unknown".to_string()
                };
                format!("pid {} {} ({owner})", holder.pid, holder.comm)
            })
            .collect::<Vec<String>>()
            .join(", ")
    }
}
