//! `stado web quality` and `stado web build` — the two steps a web product's
//! `.wisent-release.json` recipe runs on a release worker.
//!
//! Twenty-four landing sites and ten applications would otherwise carry
//! thirty-four copies of the same install-check-build-tar script, and that is
//! exactly what they carried: a `release/` directory per repository, each free
//! to drift. Both steps here read the one worker contract the release pipeline
//! already sets — `WISENT_SOURCE_DIR`, `WISENT_OUTPUT_DIR`, `WISENT_VERSION`,
//! `WISENT_PLATFORM`, `WISENT_INPUTS_DIR` — and refuse by name when one is
//! missing, because a build that guesses where its source or its output lives
//! stages bytes nobody can trace back to a commit.
//!
//! The staged tarball is reproducible: uid and gid 0, no owner names, mtime 0,
//! a fixed entry order, gzip with no timestamp of its own, and each file's
//! mode reduced to 0644 or 0755 by nothing but its execute bit. That is not
//! tidiness. `stado release` publishes an artifact under its sha256 and a
//! unit's `ServiceDeclaration` pins that hash, so a tarball whose bytes drift
//! between two builds of one commit turns every one of those pins into a claim
//! that cannot be checked.

mod contract;
mod payload;
mod steps;
mod tooling;

pub(crate) use steps::{build, quality};

/// The platform key a web product declares in `.wisent-release.json`. Both
/// steps refuse any other value: the recipe that invoked us is the web one, so
/// a different platform means the manifest names this command under a platform
/// it does not describe, and the artifact it staged would not be runnable by
/// `stado web deploy`.
const PLATFORM: &str = "web";
