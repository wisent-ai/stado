//! Read the owner-controlled env FILE a managed unit sources, and reconcile
//! the endpoints it declares against what is actually listening on the host.
//!
//! NO Python original. This module exists because of a real outage on
//! 2026-08-30. `com.wisent.always-on.weles` on charless-mac-mini crash-looped
//! with `Skarbiec at http://127.0.0.1:8785 is unreachable`; the host's
//! Skarbiec was listening on 8895; `stado service env-set` had been used twice
//! to write the right port into `$HOME/.config/weles/worker.env`, and the unit
//! kept naming the wrong one. Nothing in Stado could read that file back.
//!
//! `stado service env` already answers "what environment does the UNIT FILE
//! declare" — it parses the plist / systemd unit. That is a different file
//! from the one a launcher `.`-sources, and on this fleet the interesting
//! values live in the sourced file, not in the unit. So the fleet had a
//! configuration surface it could WRITE ([`super::service::set_env_key_on_host`])
//! and could not READ, and `stado host exec` is an exact allowlist of
//! argument-free read-only programs with no file read in it at all. An
//! operator could change that file and never see the result. That is the gap
//! this module closes.
//!
//! Three properties are deliberate, because each one is a way the previous
//! state of the world lied:
//!
//! 1. **Duplicates are reported, in file order, with the winner named.** A
//!    sourced file assigns top to bottom, so a later `KEY=` silently wins and
//!    an earlier one is dead text. `set_env_key_on_host` strips lines matching
//!    `^KEY=` and appends the new assignment, which means it cannot see an
//!    `export KEY=…` spelling of the same variable at all. A reader that
//!    collapsed the file into a map would report the value the operator wanted
//!    and hide the reason the host disagrees.
//! 2. **Redaction happens ON THE HOST.** A value this command will not show is
//!    never put on the wire, never reaches the control-plane process, and
//!    never lands in a shell history or a JSON report. Only its length
//!    crosses. A hash prefix is deliberately NOT reported: a low-entropy
//!    secret is recoverable from one, and a length is not.
//! 3. **Endpoints are shown even when the key name looks like a credential.**
//!    `WELES_CREDENTIAL_SKARBIEC_URL` is the variable this outage turned on.
//!    A name-only redaction rule hides exactly the field an operator has to
//!    verify, so the rule here is value-shaped: an inert endpoint is shown
//!    whatever the key is called, and a URL carrying userinfo
//!    (`postgres://user:pass@host/db`) is redacted whatever the key is called.
//!
//! The transport is [`host_channel::run_script`](crate::deploy::host_channel::run_script) — the same approved encrypted
//! channel `env-set`, `file-sync` and `grant-sync` already use, with the same
//! `$HOME`-confinement prelude. `host exec`'s allowlist is untouched: the
//! listener read below runs `lsof` with the same fixed flags as the
//! already-approved `lsof -nP -iTCP -sTCP:LISTEN` entry, so the two readers
//! cannot disagree about what "listening" means, and no new free-form
//! capability is introduced.
//!
//! The command is four seams: the state words below and the document model
//! they describe ([`model`]), the program that reads a host and the redaction
//! that happens there ([`script`]), what the answer MEANS ([`diagnostics`]),
//! and what a writer sees when it reads its own write back ([`readback`]).

mod diagnostics;
mod model;
mod readback;
mod script;

pub use diagnostics::{
    declared_endpoint, duplicate_keys, effective_text, endpoint_rows, endpoint_verdict, shadowing,
    EndpointRow,
};
pub use model::{Endpoint, EnvEntry, EnvFileReport, ProcListener};
pub use readback::{
    effective_entry, expectation, forward_markers, marker_holding, read_env_file, to_report,
};
pub use script::{parse_env_file, remote_env_file_script, EnvFileRequest};

/// `status` for a report that came back whole.
pub const OK_STATUS: &str = "env_file";

/// The file was a regular file the login user could read.
pub const FILE_READ: &str = "read";
/// The path resolved outside the target's home and was never opened.
pub const FILE_REFUSED_OUTSIDE_HOME: &str = "refused_outside_home";
/// The path is a symlink. Never followed: a symlink under a home directory is
/// how a read of `~/.config/x.env` becomes a read of `~/.ssh/id_ed25519`.
pub const FILE_REFUSED_SYMLINK: &str = "refused_symlink";
/// There is no regular file at the path.
pub const FILE_MISSING: &str = "missing";
/// The file exists and the login user cannot read it.
pub const FILE_UNREADABLE: &str = "unreadable";

/// The file was parsed into entries.
pub const ENTRIES_READ: &str = "read";
/// The file was readable and the parser did not finish. An empty `entries`
/// means two opposite things depending on this field, exactly as every
/// `*_state` word in [`super::host_inventory`] does: "this file declares
/// nothing" and "nobody could tell" must not look identical.
pub const ENTRIES_PARSE_FAILED: &str = "parse_failed";
/// The file was never opened, so there was nothing to parse.
pub const ENTRIES_UNREAD: &str = "unread";

/// Listeners came from `lsof`: every row carries the owning program's name.
pub const LISTENERS_READ: &str = "read";
/// Listeners came from `netstat`, which names no owner. The ports are true
/// and the `process` column is empty for a stated reason.
pub const LISTENERS_READ_WITHOUT_NAMES: &str = "read_without_names";
/// Neither reader answered. No endpoint below could be reconciled.
pub const LISTENERS_FAILED: &str = "failed";

/// A value shown verbatim, as written in the file.
pub const VALUE_SHOWN: &str = "shown";
/// A value withheld. Only its length crossed the channel.
pub const VALUE_REDACTED: &str = "redacted";
/// A value shown because the operator named this exact key with `--reveal`.
pub const VALUE_REVEALED: &str = "revealed";
/// The assignment is present and its value is the empty string.
pub const VALUE_EMPTY: &str = "empty";

// The read-back verdict for one key a caller has just written. This is how a
// writer sees its own write: `env-set` reports the value it wrote, then asks
// the host whether that key's EFFECTIVE assignment now holds it. The
// comparison is made ON THE HOST, against the same unquoting the shell would
// apply, so a secret is verified exactly without its value ever coming back.

/// No expectation was sent; the report says nothing about any key.
pub const EXPECT_NOT_ASKED: &str = "not_asked";
/// The key's effective assignment holds exactly what the caller wrote.
pub const EXPECT_MATCHED: &str = "matched";
/// The key is assigned and holds something else. Something on the host owns
/// this key and overwrote the caller.
pub const EXPECT_DIFFERS: &str = "differs";
/// The file assigns that key nowhere at all.
pub const EXPECT_ABSENT: &str = "absent";
/// The file or its assignments could not be read, so the write could not be
/// checked either way. Reported by the host on every path where it refused to
/// open the file, and substituted by [`expectation`] when the report did not
/// arrive whole — because "the write was overwritten" and "nobody could look"
/// must never collapse into one word.
pub const EXPECT_UNVERIFIED: &str = "unverified";

/// A plain `KEY=value` assignment.
pub const FORM_ASSIGNMENT: &str = "assignment";
/// An `export KEY=value` assignment. Assigns exactly like the plain form when
/// the file is sourced, and is invisible to `env-set`'s `^KEY=` rewrite —
/// which is why the two forms are reported as what they are instead of being
/// normalized into one.
pub const FORM_EXPORT: &str = "export";
/// A non-empty, non-comment line that is not an assignment at all: `. other.env`,
/// `set -a`, a stray word. Reported because a line like that changes what the
/// whole file means.
pub const FORM_UNPARSABLE: &str = "unparsable";

/// This assignment is the one a shell that sources the file ends up with.
pub const EFFECTIVE: &str = "effective";
/// A later assignment to the same key overwrites this one. Dead text.
pub const SHADOWED: &str = "shadowed";

/// Something is listening on the declared loopback port.
pub const ENDPOINT_LISTENING: &str = "listening";
/// Nothing is listening on the declared loopback port, and the socket table
/// was actually read. This is the verdict that fails the command.
pub const ENDPOINT_DEAD: &str = "dead";
/// The socket table could not be read, so the declaration was not judged.
pub const ENDPOINT_UNKNOWN: &str = "unknown";
/// The endpoint is not loopback. This host's socket table cannot answer for it.
pub const ENDPOINT_REMOTE: &str = "remote";

/// The cap on how many characters of one reported value cross the channel.
/// Larger than [`super::host_inventory::MAX_FIELD_CHARS`] because a database
/// URL or an allowlist is legitimately long, and a truncated endpoint is
/// useless for the one job this command has.
pub const MAX_VALUE_CHARS: usize = 400;

/// The cap on how many assignments are reported. A file past this is reported
/// as truncated through `entries_seen`, never silently cut.
pub const MAX_ENTRIES: usize = 400;
