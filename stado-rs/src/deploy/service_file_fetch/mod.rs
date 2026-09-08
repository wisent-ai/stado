//! Fetch one file out of a managed host's home, byte-exact and
//! integrity-checked end to end.
//!
//! NO Python original. This module exists because of what
//! [`super::service_env_file`] deliberately cannot do. That reader sanitizes
//! every value it reports — printable ASCII only, quotes and backslashes
//! replaced with `?`, long values clamped — because its job is to let an
//! operator *judge* a configuration file without a secret ever crossing the
//! channel. The consequence is that it can diagnose a file and can never
//! reproduce one byte of it.
//!
//! That gap has a name on this fleet. `$HOME/.stado/bin/weles-release-cutover`
//! on charless-mac-mini is 4357 bytes of live operator tooling that is checked
//! into no repository: it rewrote `$HOME/.config/weles/worker.env` on every
//! launchd restart for days, and the only copy of the code doing it was on the
//! host. `stado host exec` is an allowlist of argument-free read-only programs
//! with no file read in it. `service file-sync` moves a file the other way.
//! `service env-show` would have returned a redacted paraphrase. So the only
//! way to put that script under version control was to copy it off the box by
//! hand, outside the approved channel — which is the one thing the fleet-wide
//! "everything through Stado" rule exists to prevent.
//!
//! Three properties are deliberate:
//!
//! 1. **The digest is computed on the host and re-computed here, over the
//!    decoded bytes.** A base64 payload that lost a chunk in a login banner, a
//!    truncating channel, or a `stdout` cap decodes into something shorter and
//!    perfectly valid, so length alone proves nothing. The comparison is of two
//!    independently computed SHA-256s of the same bytes at the two ends of the
//!    channel, and a mismatch is [`INTEGRITY_MISMATCH`] with nothing written.
//! 2. **The confinement is [`super::service_env_file`]'s, word for word.** The
//!    command that copies a managed file must accept exactly the paths the
//!    commands that read and write one accept. A fetch with a wider rule would
//!    be an arbitrary remote-read primitive wearing a service verb's name, and
//!    `~/.config/x.env` resolving through a symlink into `~/.ssh/id_ed25519` is
//!    the exact shape that makes it one. `-L` is tested before `-f`, because
//!    `-f` follows the link.
//! 3. **A refusal is a complete report, not an error exit.** "this path is a
//!    symlink", "there is no file there" and "the channel broke" are three
//!    different findings, and a caller that cannot tell them apart cannot act
//!    on any of them.
//!
//! The transport is
//! [`host_channel::run_script`](super::host_channel::run_script) — the same
//! approved encrypted channel `env-show`, `env-set`, `file-sync` and
//! `grant-sync` use, with the operand carried base64-encoded inside the request
//! body and never in an argument vector.

mod fetch;
mod outcomes;
mod remote_script;
mod report;

pub use fetch::{digest_of, fetch_file, parse_fetch, verify};
pub use outcomes::{
    FILE_ENCODE_FAILED, FILE_MISSING, FILE_NO_HASHER, FILE_READ, FILE_REFUSED_OUTSIDE_HOME,
    FILE_REFUSED_SYMLINK, FILE_REFUSED_TOO_LARGE, FILE_UNREADABLE, INTEGRITY_MISMATCH,
    INTEGRITY_UNVERIFIED, INTEGRITY_VERIFIED, MAX_FETCH_BYTES, OK_STATUS,
};
pub use remote_script::remote_fetch_script;
pub use report::{FetchReport, FetchedFile};
