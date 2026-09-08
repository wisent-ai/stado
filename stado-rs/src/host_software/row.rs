//! One row of a software report: the program, and the detail it is stored as.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::{RELEASE, UNKNOWN};

/// One program on one host, as that host reported it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostSoftware {
    /// The program's basename, as an operator would say it.
    pub name: String,
    /// Where it is on the host. Absolute, and the one field here that may
    /// contain a space.
    pub path: String,
    /// What the program says it is, or [`UNKNOWN`].
    pub version: String,
    /// What the bytes are, lowercase hex, or [`UNKNOWN`] when the host has no
    /// way to compute one.
    pub sha256: String,
    /// [`RELEASE`] or [`UNMANAGED`](super::UNMANAGED), as the host's own digest
    /// comparison decided. A word from a newer reporter is carried through
    /// verbatim rather than rounded to whichever of these two it resembles.
    pub provenance: String,
}

impl HostSoftware {
    pub fn is_release(&self) -> bool {
        self.provenance == RELEASE
    }

    /// The four fields the fact name does not carry, in the shape
    /// [`Observation`](crate::observations::Observation)'s detail keeps them.
    ///
    /// `path` last and unquoted, for the reason the wire format puts it last: it
    /// is the only value that may contain a space, and every reader takes the
    /// rest of the line for it.
    pub(super) fn detail(&self) -> String {
        format!(
            "version={} sha256={} provenance={} path={}",
            self.version, self.sha256, self.provenance, self.path
        )
    }

    /// The inverse of [`Self::detail`], against a name taken from the fact.
    ///
    /// `None` for anything that is not a whole row. A missing path or a missing
    /// provenance is not a row with a default; completing it would put a
    /// fabricated `unmanaged` in front of an operator about bytes nothing read.
    pub(super) fn from_detail(name: &str, detail: &str) -> Option<Self> {
        let (head, path) = match detail.split_once("path=") {
            Some((head, path)) => (head, path.trim()),
            None => (detail, ""),
        };
        let mut row = Self {
            name: name.to_string(),
            path: path.to_string(),
            version: UNKNOWN.to_string(),
            sha256: UNKNOWN.to_string(),
            provenance: String::new(),
        };
        for token in head.split_whitespace() {
            if let Some(value) = token.strip_prefix("version=") {
                row.version = value.to_string();
            } else if let Some(value) = token.strip_prefix("sha256=") {
                row.sha256 = value.to_string();
            } else if let Some(value) = token.strip_prefix("provenance=") {
                row.provenance = value.to_string();
            }
        }
        if row.path.is_empty() || row.provenance.is_empty() {
            return None;
        }
        Some(row)
    }

    pub fn json(&self) -> Value {
        json!({
            "name": self.name,
            "path": self.path,
            "version": self.version,
            "sha256": self.sha256,
            "provenance": self.provenance,
        })
    }

    /// The digest, short enough to read inside a sentence and long enough to
    /// look up. A full 64 characters mid-sentence is a sentence nobody reads.
    pub(super) fn short_digest(&self) -> &str {
        self.sha256.get(..12).unwrap_or(&self.sha256)
    }
}
