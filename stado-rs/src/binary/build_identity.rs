//! Which tree this binary was built from.
//!
//! The semantic version does not identify content. One version string can
//! name several materially different trees of this crate at once: the binary
//! the fleet is running, separate commits each declaring that version in
//! `Cargo.toml`, and a local build with yet another combination. With no
//! release object for the version to tell them apart, only a coordinate
//! claim, establishing what the running control plane carries means reading
//! string literals and mangled symbols out of the binary with `strings` and
//! `nm`.
//!
//! [`BUILD_IDENTITY`] is the answer to that question as a read. It is what
//! `stado --version` prints and what the agent publishes for itself, so every
//! host says which tree it is running without anybody dissecting a binary.
//!
//! `build.rs` guarantees `STADO_SOURCE_REVISION` is set in every build
//! context, including one with no git metadata, where it is
//! [`UNKNOWN_REVISION`]. See that file for the resolution order and for why a
//! build that cannot name a revision still builds.

/// The crate's semantic version, unchanged and still comparable. Version
/// ordering — `release_agent`'s `minimum_stado_version` check, the agent's
/// release-handoff comparison, `self_update` — reads this and never
/// [`BUILD_IDENTITY`], because a revision has no order.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The revision this binary was built from: a full 40-digit lowercase commit,
/// optionally suffixed `-dirty`, or [`UNKNOWN_REVISION`].
pub const SOURCE_REVISION: &str = env!("STADO_SOURCE_REVISION");

/// What [`SOURCE_REVISION`] reads as when no build context could name one.
/// A value, never an error: a tarball build is a legitimate build.
pub const UNKNOWN_REVISION: &str = "unknown";

/// Version and revision as one line, for anywhere a human or a log reads
/// "which build is this": `0.15.2 (rev <40 lowercase hex digits>)`.
///
/// `concat!` over `env!` keeps this a `&'static str` literal, which is what
/// clap's `version` needs and what avoids a lazily-initialised global for a
/// value fixed at compile time.
pub const BUILD_IDENTITY: &str = concat!(
    env!("CARGO_PKG_VERSION"),
    " (rev ",
    env!("STADO_SOURCE_REVISION"),
    ")"
);

/// Whether this build can name the tree it came from. False for a tarball or
/// a history-less checkout; a caller that wants to insist on provenance asks
/// here rather than string-matching [`BUILD_IDENTITY`].
pub fn revision_known() -> bool {
    SOURCE_REVISION != UNKNOWN_REVISION
}
