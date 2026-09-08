//! The env document model: one parsed line, one listening socket, the whole
//! report a host answers with, and the endpoint a value declares.
//!
//! The state words every field below is documented against live one module
//! up, beside each other, because they are the vocabulary this command
//! answers in.

use serde::{Deserialize, Serialize};

/// One line of the env file that assigns something, or fails to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnvEntry {
    /// 1-based line number in the file, so the operator can go to the line.
    pub line: u32,
    /// [`FORM_ASSIGNMENT`](super::FORM_ASSIGNMENT), [`FORM_EXPORT`](super::FORM_EXPORT) or [`FORM_UNPARSABLE`](super::FORM_UNPARSABLE).
    pub form: String,
    /// The variable name, empty for [`FORM_UNPARSABLE`](super::FORM_UNPARSABLE).
    pub key: String,
    /// [`VALUE_SHOWN`](super::VALUE_SHOWN), [`VALUE_REDACTED`](super::VALUE_REDACTED), [`VALUE_REVEALED`](super::VALUE_REVEALED) or
    /// [`VALUE_EMPTY`](super::VALUE_EMPTY).
    pub value_state: String,
    /// The text after `=`, exactly as written (quotes included), sanitized to
    /// printable ASCII. Empty whenever `value_state` is [`VALUE_REDACTED`](super::VALUE_REDACTED),
    /// and the state beside it says so.
    pub value: String,
    /// How many characters the value has, quotes removed. Reported for a
    /// redacted value too: "the token is 0 characters long" is a finding.
    pub chars: u32,
}

/// One listening TCP socket, and the program holding it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProcListener {
    /// `127.0.0.1`, `::1`, or `*` for a socket bound to every interface,
    /// which answers on loopback too.
    pub address: String,
    pub port: u32,
    pub pid: u32,
    /// The program name `lsof` reported, empty under
    /// [`LISTENERS_READ_WITHOUT_NAMES`](super::LISTENERS_READ_WITHOUT_NAMES). `lsof` truncates this to nine
    /// characters; the flags are held identical to the already-approved
    /// `host exec` entry rather than widened, so both readers report the same
    /// name for the same process.
    pub process: String,
}

/// Everything the remote script reported about one env file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnvFileReport {
    /// The absolute path the host resolved, empty when it refused to resolve one.
    pub path: String,
    /// [`FILE_READ`](super::FILE_READ) or one of the refusals above.
    pub file_state: String,
    /// Why the file state is what it is, in the host's own words.
    pub detail: String,
    /// Permission bits in octal (`600`), or `unknown`.
    pub mode: String,
    /// No group and no other bits. An env file the group can read is a
    /// finding, not a cosmetic detail.
    pub owner_only: bool,
    pub bytes: u64,
    /// [`ENTRIES_READ`](super::ENTRIES_READ), [`ENTRIES_PARSE_FAILED`](super::ENTRIES_PARSE_FAILED) or [`ENTRIES_UNREAD`](super::ENTRIES_UNREAD).
    pub entries_state: String,
    pub entries: Vec<EnvEntry>,
    /// How many assignments the file has, including any past [`MAX_ENTRIES`](super::MAX_ENTRIES)
    /// that `entries` therefore does not list.
    pub entries_seen: u32,
    /// [`EXPECT_NOT_ASKED`](super::EXPECT_NOT_ASKED), [`EXPECT_MATCHED`](super::EXPECT_MATCHED), [`EXPECT_DIFFERS`](super::EXPECT_DIFFERS) or
    /// [`EXPECT_ABSENT`](super::EXPECT_ABSENT) — the host's verdict on the one key the caller asked
    /// it to check, decided against that key's LAST assignment because that is
    /// the one a sourced file leaves behind.
    pub expected: String,
    /// [`LISTENERS_READ`](super::LISTENERS_READ), [`LISTENERS_READ_WITHOUT_NAMES`](super::LISTENERS_READ_WITHOUT_NAMES) or
    /// [`LISTENERS_FAILED`](super::LISTENERS_FAILED).
    pub listeners_state: String,
    pub listeners: Vec<ProcListener>,
}

/// A loopback endpoint one env value declares.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Endpoint {
    pub port: u32,
    /// Whether the authority names this machine. Only a loopback endpoint can
    /// be reconciled against this host's own socket table.
    pub loopback: bool,
}
