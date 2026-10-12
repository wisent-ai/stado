//! How a host-side removal ended.
//!
//! Every guarded delete Stado runs on a host (a file, a run directory, a
//! superseded LaunchAgent plist) reports one of these. The report's word is
//! parsed into this type once, where the report is read; a word outside it
//! is a malformed report, refused with serde's own reason.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RemovalStatus {
    /// The thing was deleted and is verified gone.
    Removed,
    /// There was nothing to delete.
    Absent,
    /// A guard refused the path; nothing was attempted.
    Refused,
    /// The delete was attempted and did not leave the thing gone.
    Failed,
}

impl RemovalStatus {
    /// The removal left the thing gone: it was deleted, or it was not there.
    pub fn gone(self) -> bool {
        matches!(self, Self::Removed | Self::Absent)
    }

    /// One report word, refused by name when it is not a removal status.
    pub fn parse(word: &str) -> Result<Self, String> {
        Self::deserialize(serde_json::Value::String(word.to_string()))
            .map_err(|error| format!("removal status: {error}"))
    }
}

/// The report word, from the same serde mapping.
impl std::fmt::Display for RemovalStatus {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match serde_json::to_value(self) {
            Ok(serde_json::Value::String(word)) => formatter.write_str(&word),
            _ => Err(std::fmt::Error),
        }
    }
}
