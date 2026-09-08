//! `stado host inventory TARGET` — what stado actually installed on one
//! registry host, what that host's forward markers claim, and whether the
//! two agree.
//!
//! NO Python original. This exists because of a real diagnosis that could
//! not be finished with the shipped commands: on `control-host` the
//! marker `$HOME/.stado/forwards/stado-weles-api.url` said
//! `http://127.0.0.1:8766` while the admission API was listening on `8794`.
//! Nothing in the fleet noticed, because nothing in the fleet reads the
//! markers. Finding it took a raw `ssh user@ip '<inline script>'` with a
//! hardcoded address, and answering "does that port still exist" reached
//! for `pgrep -fl` and `printenv` — the two things
//! [`crate::deploy::host_exec`]'s allowlist deliberately does not offer,
//! because process arguments and environments are where the secrets are.
//!
//! The fix is NOT a wider allowlist. `host exec` passes a program's output
//! through to the operator's terminal untouched, and the three facts missing
//! here are read out of files under `~/.stado` that a corrupt or hostile
//! writer also reaches. So this is a separate command with its own, narrower
//! contract:
//!
//! - it takes a registry TARGET NAME and nothing else — no path, no file
//!   name, no port, no pattern. There is no way to point it at something;
//!   it is a fixed question, not a parameterizable probe;
//! - its remote program is one compile-time script with no interpolation
//!   in it at all, run over the shared channel
//!   ([`crate::deploy::host_channel::run_script`]), the same way
//!   `route open --remote` writes its marker;
//! - every value it reads off the host is reduced to a JSON-inert
//!   character set and capped in length on BOTH sides, so a corrupt or
//!   hostile file under `~/.stado` cannot push arbitrary text into an
//!   operator's terminal;
//! - it refuses to follow a symlink, at the managed binaries, the markers
//!   or the vault files, and reports the refusal instead of reading through
//!   it.
//!
//! What it will never show: process arguments, process environments, vault
//! or secret file contents, tokens, or anything read through `lsof` or
//! `pgrep -f`. Listener ownership comes from `netstat -anv -p tcp`, the
//! program `host exec` already justifies as safe precisely because it reads
//! the kernel socket table and no process's argv.
//!
//! The point of the command is the last section, not the first three. A
//! table of markers next to a table of listeners is still homework; the
//! report answers the question directly, per marker and in aggregate:
//! `matched` when something is listening on the port the marker names,
//! `stale` when nothing is. `reconciliation.stale_markers` is the sentence
//! an operator actually needs.
//!
//! The same question is asked of Skarbiec: `vaults` is
//! `$HOME/.stado/*.vault.json`, `vault_sidecars` is everything else under
//! `$HOME/.stado/*.vault*.json` — snapshots, pre-migration copies,
//! `*.acquisitions.json`. Keeping them apart is operational, not tidy: the
//! active vault is state and a sidecar is history, and an operator who
//! confuses the two edits the wrong file.
//!
//! That section is METADATA ONLY, and that is a boundary rather than an
//! omission. It reports that a vault exists, how large it is, its mode, and
//! whether anything but its owner can read it. It never opens one: no
//! ciphertext, no item id, no consumer name, no token. `stat(2)` answers
//! "which vaults are on this host" completely, so nothing here needs
//! `open(2)`, and a diagnostic command that reads secret files is a
//! diagnostic command that leaks them into terminals and logs.
//! `reconciliation.vaults_not_owner_only` is the finding that matters: a
//! vault the group can read is an incident.
//!
//! Cargo uses the same fixed, metadata-only boundary. `cargo.home` uses lstat
//! on `$HOME/.cargo` and preserves its link target; `cargo.bin` is the fixed
//! direct `bin` child even when Cargo home is a symlink; and `cargo.entries`
//! names every direct child, including dotfiles, with type, mode, numeric
//! ownership, size, mtime, and symlink text. No operator path enters the
//! script and no file body is opened. `complete` is false after any refused
//! or partial traversal, unreadable or malformed metadata, sanitized name or
//! link, or output cap, so a prefix can never masquerade as the whole
//! directory.
//!
//! Reporting drift is not the same as failing on it. A host with a forward
//! that was deliberately torn down is not a broken host, so `status` stays
//! [`OK_STATUS`] whenever the inventory was collected; the drift is in the
//! report, loudly, and the exit status stays usable for "just give me the
//! facts".

mod reads;
mod report;

pub use reads::{
    declaration_verdict, declared_adapter, declared_endpoint, marker_port, remote_inventory_script,
    reported_version, verdict, version_verdict, CargoInventory, FilesystemMetadata, ForwardMarker,
    Listener, ManagedBinary, ServiceArtifact, Subcommand, VaultFile, REMOTE_INVENTORY_BODY,
};
pub use report::{inventory_host, inventory_target, parse_inventory, to_report, Inventory};

/// `status` for an inventory that was collected. Whether it found drift is
/// a question the report answers, not a question the exit status answers.
pub const OK_STATUS: &str = "inventory";

/// The cap, in characters, on every string this command reports.
///
/// The remote script caps its own fields at the same number; this side caps
/// again because the far side is whatever answered the ssh connection, and a
/// guarantee that only holds when the remote behaves is not a guarantee.
pub const MAX_FIELD_CHARS: usize = 200;

/// Appended to a value this side had to clip, so a truncated string is never
/// mistaken for a whole one. Counted inside [`MAX_FIELD_CHARS`].
const ELLIPSIS: &str = "...";

/// A marker the script read successfully. Any other state means the file was
/// refused, not that it was empty.
pub const MARKER_READ: &str = "read";

/// The two sides of a comparison agree. Shared by all three axes: a marker
/// whose port something is listening on, a marker that names the endpoint
/// the registry declares, and a binary at the version the registry
/// requires.
pub const MATCHED: &str = "matched";
/// The marker exists and nothing is listening on the port it names. This is
/// the `8766` / `8794` case that started this command.
pub const STALE: &str = "stale";
/// The marker could not be turned into a port to check: it was refused as a
/// symlink or a non-regular file, or its contents are not a loopback URL.
pub const UNREADABLE: &str = "unreadable";
/// There was nothing to compare against, so no verdict was reached. For a
/// marker: it names a port and the host's socket table was not read. This
/// is not [`STALE`] — an empty listener table would otherwise turn one
/// failed `netstat` into a report that every forward on the host is dead.
/// For a binary: the registry declares a version and the host reported none
/// that could be read, which is not the same finding as the two disagreeing.
pub const UNKNOWN: &str = "unknown";

/// The installed binary is OLDER than the version the registry declares.
///
/// This axis is independent of the two marker axes below it, and it is the
/// one the fleet was blind to: `host inventory` could always read that
/// `operator-host` runs `stado 0.4.392`, and had nothing to say about
/// whether that is the version it is supposed to run.
pub const BEHIND: &str = "behind";
/// The installed binary is NEWER than the version the registry declares.
/// Not a lesser finding than [`BEHIND`]: a host running ahead of the
/// declaration means the declaration was never updated, and the next host
/// brought to the declared version is a host taken backwards.
pub const AHEAD: &str = "ahead";
/// Declared and installed differ, and at least one of them is not three
/// dot-separated numbers, so there is no older-or-newer to report. Saying
/// `mismatched` beats ordering two strings whose ordering is invented.
pub const MISMATCHED: &str = "mismatched";
/// The registry declares nothing for this binary or this marker, so there
/// is no target state to hold the host to. Reported as its own word rather
/// than folded into [`MATCHED`]: an undeclared thing is not a verified
/// thing, and a fleet whose registry is silent should read as unverified.
pub const UNDECLARED: &str = "undeclared";
/// The marker and the registry name DIFFERENT endpoints for this host.
///
/// The second, independent reconciliation axis. A marker can be [`MATCHED`]
/// against the socket table and `disagrees` against the registry at the
/// same time, and that combination is the dangerous one: something is
/// listening, so nothing looks broken, and it is not the endpoint the
/// directory sends consumers to. `skarbiec-weles` on `control-host` is
/// exactly that — the marker says `8895`, something answers on `8895`, and
/// the registry declares `19095`.
pub const DISAGREES: &str = "disagrees";

/// [`ManagedBinary::version_state`] when the host actually answered with a
/// version. Every other state is a reason the `version` field is blank, so
/// it is the only state whose `version` may be compared to a declaration.
pub const VERSION_REPORTED: &str = "reported";

/// The remote script's own sanitizer answered its fixed probe correctly, so
/// every string in the report was reduced by a working sanitizer.
pub const SANITIZER_OK: &str = "ok";
/// The sanitizer did not answer its own probe. Every string in the report is
/// then suspect, which is a host fault to state outright rather than
/// something for an operator to infer from a table of blank names — the
/// exact failure this state exists because of.
pub const SANITIZER_BROKEN: &str = "broken";

/// `netstat -anv -p tcp` answered and its output was parsed.
pub const LISTENERS_READ: &str = "read";
/// `netstat` did not answer. The listener table is empty because it could
/// not be read, not because nothing is listening.
pub const LISTENERS_FAILED: &str = "failed";

/// The cap on how many files each vault section reports.
///
/// The remote script caps at the same number. A `~/.stado` with a thousand
/// files must not produce an unbounded report, and the script counts
/// everything it matched into `vaults_seen` / `vault_sidecars_seen`, so the
/// cap shows up as a number the report states rather than as a silent cut.
pub const MAX_VAULT_FILES: usize = 64;
/// The most Cargo bin entries one report will carry.
///
/// The script still counts every matching directory member. When this cap is
/// exceeded, `entries_complete` is false rather than silently presenting the
/// prefix as the whole directory.
pub const MAX_CARGO_BIN_ENTRIES: usize = 512;

/// A vault path that is a regular file. Its METADATA was read; its contents
/// were not, and there is no state in which they would be.
pub const VAULT_REGULAR: &str = "regular";
/// A vault path that is a symlink. The link is reported and never followed:
/// the size and mode belong to the link, not to whatever it points at.
pub const VAULT_REFUSED_SYMLINK: &str = "refused_symlink";
/// A vault path that exists and is neither a symlink nor a regular file.
pub const VAULT_REFUSED_NOT_REGULAR: &str = "refused_not_regular";
