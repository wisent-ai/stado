//! The software this host has installed: one record per declared program
//! product, the fixed subcommand probe, and the comparison of an installed
//! version against the one the registry declares for this host.

use std::cmp::Ordering;

use serde::{Deserialize, Serialize};

use super::super::*;

/// One declared program product under its install root.
///
/// `version_state` is always an explicit word — `reported`, `missing`,
/// `not_executable`, `version_failed`, `version_empty`,
/// `version_unparsable`, `refused_symlink`, `refused_not_regular`. An empty
/// `version` never has to be interpreted, because the state next to it
/// already says why it is empty.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManagedBinary {
    pub name: String,
    pub state: String,
    pub regular_file: bool,
    pub executable: bool,
    pub version_state: String,
    pub version: String,
}

/// Whether the installed `stado` knows one fixed subcommand path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Subcommand {
    pub name: String,
    /// `present`, `absent`, `unavailable` when there was no usable binary to
    /// ask, or `probe_failed` when the binary was there and never got to
    /// answer — a host out of process slots must not be reported as a host
    /// running an old `stado`.
    pub state: String,
}

/// The bare semantic version inside one [`ManagedBinary::version`] field.
///
/// A declared program answers in the shape its declaration names, and this
/// function reduces only its own documented banner shape to the bare
/// coordinate:
///
/// - `stado --version` prints
///   `stado 0.15.5 (rev <40 lowercase hex digits>)`; releases before the full
///   source-identity cutover used a 12-digit revision. The name and either
///   exact build-identity suffix are removed;
/// - `skarbiec version` prints a JSON object, and the remote script has
///   already pulled its `version` member out, so it arrives bare (`0.1.3`).
///
/// Anything else is returned untouched. Guessing a number out of an
/// unfamiliar banner — taking the last word, the first digit run — is how a
/// build string becomes a version, and a wrong version compares cleanly
/// against a declaration and reports the wrong verdict with confidence.
pub fn reported_version<'a>(binary: &str, version: &'a str) -> Option<&'a str> {
    let trimmed = version.trim();
    let named = match trimmed.strip_prefix(binary) {
        Some(rest) if rest.is_empty() || rest.starts_with(char::is_whitespace) => rest.trim_start(),
        _ => trimmed,
    };
    let bare = if binary == "stado" {
        named
            .strip_suffix(')')
            .and_then(|banner| banner.split_once(" (rev "))
            .and_then(|(version, revision)| {
                let core = revision.strip_suffix("-dirty").unwrap_or(revision);
                (revision == crate::build_identity::UNKNOWN_REVISION
                    || ([12, 40].contains(&core.len())
                        && core
                            .bytes()
                            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))))
                .then_some(version)
            })
            .unwrap_or(named)
    } else {
        named
    };
    if bare.is_empty() {
        None
    } else {
        Some(bare)
    }
}

/// A version as three dot-separated numbers, or `None` for everything else.
///
/// Deliberately strict: exactly three components, each a plain integer, no
/// prerelease and no build metadata. A shape this does not recognize is not
/// ordered at all — it falls through to exact equality — because inventing
/// an ordering for `0.5.1-rc2` produces a confident `behind` or `ahead`
/// that nobody can check.
fn version_triple(value: &str) -> Option<(u64, u64, u64)> {
    let mut parts = value.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = parts.next()?.parse().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some((major, minor, patch))
}

/// One managed binary's verdict against the version the registry declares
/// for this host: [`MATCHED`], [`BEHIND`], [`AHEAD`], [`MISMATCHED`],
/// [`UNDECLARED`] or [`UNKNOWN`].
///
/// Only a binary whose `version_state` is [`VERSION_REPORTED`] has a version
/// to compare. Every other state means the `version` field is blank for a
/// stated reason, and comparing a blank against a declaration would report
/// a missing binary as a version disagreement.
pub fn version_verdict(binary: &ManagedBinary, declared: Option<&str>) -> &'static str {
    let Some(declared) = declared else {
        return UNDECLARED;
    };
    if binary.version_state != VERSION_REPORTED {
        return UNKNOWN;
    }
    let Some(installed) = reported_version(&binary.name, &binary.version) else {
        return UNKNOWN;
    };
    match (version_triple(installed), version_triple(declared)) {
        (Some(installed), Some(declared)) => match installed.cmp(&declared) {
            Ordering::Less => BEHIND,
            Ordering::Equal => MATCHED,
            Ordering::Greater => AHEAD,
        },
        _ if installed == declared.trim() => MATCHED,
        _ => MISMATCHED,
    }
}
