//! Version primitives this process needs in-process: ordering, and the
//! canonical-coordinate check.
//!
//! The rule that decides a release — what class of change happened and which slot
//! advances — is not here and must not come back. It lives once for the whole fleet
//! at <https://github.com/lbartoszcze/AutoVersion>, with its contract in that
//! repository's fixtures, and callers ask it rather than reimplement it.
//!
//! Ordering stays because `self_update` compares a configured version against the
//! installed one on a hot path, and the coordinate check stays because configuration
//! is validated before anything else runs. Both are small enough to be a port of the
//! shared specification rather than a rival implementation of it.

/// One token of Python `_version_tuple`: (0, int) for numeric tokens,
/// (1, str) otherwise — numeric tokens always sort before string tokens.
///
/// Release coordinates across this fleet are not all semantic triples
/// (`147.0.7727.108-weles.1` is a real one), so ordering has to work on
/// arbitrary dotted and dashed tokens even though bumping does not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VersionToken {
    Num(i64),
    Str(String),
}

impl Ord for VersionToken {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        use std::cmp::Ordering;
        match (self, other) {
            (VersionToken::Num(a), VersionToken::Num(b)) => a.cmp(b),
            (VersionToken::Str(a), VersionToken::Str(b)) => a.cmp(b),
            // Python (0, int) < (1, str).
            (VersionToken::Num(_), VersionToken::Str(_)) => Ordering::Less,
            (VersionToken::Str(_), VersionToken::Num(_)) => Ordering::Greater,
        }
    }
}

impl PartialOrd for VersionToken {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

/// Python `_version_tuple`: split on "." and "-", numeric tokens as ints.
/// Slice comparison matches Python tuple semantics (a proper prefix sorts
/// before its extension: 1.0 < 1.0.1).
pub fn version_tuple(v: &str) -> Vec<VersionToken> {
    v.replace('-', ".")
        .split('.')
        .map(|token| match token.parse::<i64>() {
            Ok(n) => VersionToken::Num(n),
            // i64 overflow lands here too; real release versions never do.
            Err(_) => VersionToken::Str(token.to_string()),
        })
        .collect()
}

/// True when `latest` is strictly newer than `installed`.
pub fn version_newer(installed: &str, latest: &str) -> bool {
    version_tuple(installed) < version_tuple(latest)
}

/// A coordinate segment usable in `stado://releases/<product>/<version>/<platform>/`:
/// non-empty, free of surrounding whitespace, and restricted to characters that
/// survive a URL path and a filesystem key unchanged.
///
/// This is what stops a mutable alias from being addressed as if it were a
/// version. `latest` passes it — it is a legal segment — which is why the absence
/// of any code that *resolves* an alias matters more than the charset.
pub fn canonical_coordinate(value: &str) -> bool {
    !value.is_empty()
        && value.trim() == value
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

// The release *decision* used to live here: a version triple, a change
// classification, a surface diff and the slot mapping. It has been removed.
//
// That rule now has one home for the whole fleet, https://github.com/lbartoszcze/AutoVersion,
// with its contract in that repository's FIXTURES.md. Keeping a second
// implementation here is what the consolidation exists to end: nothing fails when
// two copies disagree, because each release looks correct on its own. This copy
// disagreed with the one in the public Python package about what advances a minor
// slot and about what a breaking change does while the major slot is zero.
//
// What stays is what a Rust process genuinely needs in-process and cannot ask a
// subprocess for on every call:
//
// - ordering, used at runtime to decide whether a configured version is newer than
//   the installed one;
// - the canonical-coordinate check, used when validating configuration.
//
// Under the fleet's chosen shape — one specification, several small ports kept
// honest by shared fixtures — those two are a port, not a copy. They are not yet
// checked against the fixtures; doing that needs a test, which is a separate change.
