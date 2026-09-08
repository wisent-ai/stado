//! What the host said about one fetched file, and what this process made of
//! it once the payload was decoded and judged.

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use super::outcomes::{FILE_READ, INTEGRITY_VERIFIED, OK_STATUS};
use crate::targets::ComputeTarget;

/// Everything the remote script reported about one fetched file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FetchReport {
    /// The absolute path on the host, as the host resolved it. Empty on a
    /// refusal that happened before any path was accepted.
    pub path: String,
    /// [`FILE_READ`] or one of the refusal words above.
    pub file_state: String,
    /// Why, in the host's words, for any state that is not [`FILE_READ`].
    pub detail: String,
    /// The file's permission bits as the host prints them (`600`, `700`), or
    /// `unknown`.
    pub mode: String,
    /// Whether the mode denies group and other entirely.
    pub owner_only: bool,
    /// The file's size in bytes, as `stat` reported it before the read.
    pub bytes: u64,
    /// The SHA-256 the HOST computed over the file itself, lowercase hex.
    /// Empty when nothing was read.
    pub digest: String,
    /// The file's bytes, base64, on one line. Empty when nothing was read.
    #[serde(default)]
    pub content_b64: String,
}

/// A fetch that arrived, decoded and verified: the bytes, and everything the
/// host said about the file they came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchedFile {
    /// The host's own report.
    pub report: FetchReport,
    /// The decoded bytes, byte-exact.
    pub content: Vec<u8>,
    /// The SHA-256 recomputed HERE over [`Self::content`], lowercase hex.
    pub local_digest: String,
    /// [`INTEGRITY_VERIFIED`],
    /// [`INTEGRITY_MISMATCH`](super::INTEGRITY_MISMATCH), or
    /// [`INTEGRITY_UNVERIFIED`](super::INTEGRITY_UNVERIFIED).
    pub integrity: &'static str,
}

impl FetchedFile {
    /// True only when the file came back whole and both ends agree on its
    /// bytes.
    pub fn ok(&self) -> bool {
        self.report.file_state == FILE_READ && self.integrity == INTEGRITY_VERIFIED
    }

    /// Why this fetch cannot be believed, or `None` when it can.
    ///
    /// A file that was never opened and a file whose digests disagree are
    /// different failures, and both are failures: this is the one place that
    /// decides so, and every caller reads it rather than re-deriving it.
    pub fn failure(&self, host: &str) -> Option<String> {
        if self.report.file_state != FILE_READ {
            return Some(format!(
                "{host}: {} — {}",
                self.report.file_state,
                if self.report.detail.is_empty() {
                    "no detail"
                } else {
                    &self.report.detail
                }
            ));
        }
        if self.integrity != INTEGRITY_VERIFIED {
            return Some(format!(
                "{host}: the file arrived and its bytes are not what the host hashed \
                 (host {}, local {}); nothing was written",
                if self.report.digest.is_empty() {
                    "-"
                } else {
                    &self.report.digest
                },
                if self.local_digest.is_empty() {
                    "-"
                } else {
                    &self.local_digest
                }
            ));
        }
        None
    }

    /// The fetch as a `--json` report, in
    /// [`super::host_inventory`](crate::deploy::host_inventory)'s report
    /// shape.
    ///
    /// The bytes are NOT in it. A JSON report is what an operator pastes into
    /// a ticket and what a script pipes into a log; the payload's destination
    /// is the file the caller asked for, and duplicating it here would be a
    /// second uncontrolled copy of exactly the content this command exists to
    /// handle carefully.
    pub fn to_report(&self, target: &ComputeTarget, unit: &str) -> Map<String, Value> {
        let mut object = Map::new();
        object.insert("host".to_string(), json!(target.name));
        object.insert("unit".to_string(), json!(unit));
        object.insert("status".to_string(), json!(OK_STATUS));
        object.insert("path".to_string(), json!(self.report.path));
        object.insert("file_state".to_string(), json!(self.report.file_state));
        object.insert("detail".to_string(), json!(self.report.detail));
        object.insert("mode".to_string(), json!(self.report.mode));
        object.insert("owner_only".to_string(), json!(self.report.owner_only));
        object.insert("bytes".to_string(), json!(self.report.bytes));
        object.insert("fetched_bytes".to_string(), json!(self.content.len()));
        object.insert("host_digest".to_string(), json!(self.report.digest));
        object.insert("local_digest".to_string(), json!(self.local_digest));
        object.insert("integrity".to_string(), json!(self.integrity));
        object
    }
}
