//! Which tree this binary was built from.
//!
//! The semantic version does not identify content. On 2026-09-03 `0.14.6`
//! named four materially different trees of this crate: the binary the fleet
//! was running (without the janitor workload-hold fix or the builder
//! claimability fix), two separate commits each declaring `version = "0.14.6"`
//! in `Cargo.toml`, and a local build with a fourth combination. No release
//! object existed for `0.14.6` to tell them apart, only a coordinate claim, so
//! establishing what the running control plane carried meant reading string
//! literals and mangled symbols out of the binary with `strings` and `nm`.
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

/// The revision this binary was built from: twelve hex digits, optionally
/// suffixed `-dirty`, or [`UNKNOWN_REVISION`].
pub const SOURCE_REVISION: &str = env!("STADO_SOURCE_REVISION");

/// What [`SOURCE_REVISION`] reads as when no build context could name one.
/// A value, never an error: a tarball build is a legitimate build.
pub const UNKNOWN_REVISION: &str = "unknown";

/// Version and revision as one line, for anywhere a human or a log reads
/// "which build is this": `0.14.8 (rev a1b2c3d4e5f6)`.
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The defect this module exists for: an identity that carries only the
    /// version answers nothing, because several trees share one version.
    #[test]
    fn the_identity_carries_both_the_version_and_the_revision() {
        assert!(
            BUILD_IDENTITY.contains(VERSION),
            "{BUILD_IDENTITY} does not name its version"
        );
        assert!(
            BUILD_IDENTITY.contains(SOURCE_REVISION),
            "{BUILD_IDENTITY} does not name the tree it was built from, which is the \
             whole point: 0.14.6 named four different trees and the version could not \
             tell them apart"
        );
        assert_ne!(
            BUILD_IDENTITY, VERSION,
            "the identity is indistinguishable from the bare version"
        );
    }

    /// A build context with no git metadata must still produce a usable
    /// identity rather than an empty one or a panic.
    #[test]
    fn the_revision_is_always_some_stated_value() {
        assert!(!SOURCE_REVISION.is_empty(), "the revision is empty");
        assert_eq!(
            SOURCE_REVISION.trim(),
            SOURCE_REVISION,
            "the revision carries surrounding whitespace"
        );
        assert!(
            !SOURCE_REVISION.contains('\n'),
            "the revision spans lines, so it cannot sit on a version line"
        );
    }

    /// Either the sentinel, or a short hex revision with an optional dirty
    /// marker. Anything else means `build.rs` passed through something it
    /// should have normalised — a branch name, a tag, an error message.
    #[test]
    fn the_revision_has_one_of_its_two_declared_shapes() {
        if !revision_known() {
            assert_eq!(SOURCE_REVISION, UNKNOWN_REVISION);
            return;
        }
        let core = SOURCE_REVISION
            .strip_suffix("-dirty")
            .unwrap_or(SOURCE_REVISION);
        assert!(
            core.len() >= 7 && core.len() <= 40,
            "{core:?} is not a git revision length"
        );
        assert!(
            core.chars().all(|character| character.is_ascii_hexdigit()),
            "{core:?} is not hexadecimal, so it is not a revision"
        );
    }

    /// The version stays clean, because ordering depends on it. A
    /// `minimum_stado_version` comparison against `0.14.8 (rev abc)` would
    /// not parse.
    #[test]
    fn the_version_is_not_polluted_by_the_revision() {
        assert!(
            !VERSION.contains("rev"),
            "{VERSION} carries build identity and can no longer be ordered"
        );
        assert!(
            VERSION
                .chars()
                .all(|character| character.is_ascii_digit() || character == '.'),
            "{VERSION} is not a bare semantic version"
        );
    }
}
