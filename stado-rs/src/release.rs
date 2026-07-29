//! The one place a release version is parsed, compared, and decided.
//!
//! Before this module the rules lived in four places and none of them answered
//! the question anyone actually asks. `providers::local::version_check` knew how
//! to *order* two versions. `self_update` knew what a *canonical* coordinate
//! looked like. `cli::storage` knew that the `releases` namespace is create-only.
//! A shell script knew where the number was written down. Nothing knew when
//! `0.1.0` should become `0.1.1` rather than `0.2.0`, so that decision was made
//! by whoever was publishing, from memory.
//!
//! Two things are deliberately separate here:
//!
//! - **What changed** ([`Change`]) is a fact about the software, derived from
//!   comparing the published surface against the candidate one.
//! - **Which number moves** depends on the current version, because for `0.x`
//!   versions the compatibility boundary is the minor slot, not the major one.
//!   That is Cargo's rule for this ecosystem, not a local invention, and it is
//!   why a breaking change to `0.1.0` produces `0.2.0` instead of `1.0.0`.
//!
//! The mapping, in full:
//!
//! | Change | `0.x.y` | `x.y.z` where x >= 1 |
//! | --- | --- | --- |
//! | [`Change::Breaking`] | `0.(y+1).0` | `(x+1).0.0` |
//! | [`Change::Additive`] | `0.x.(z+1)` | `x.(y+1).0` |
//! | [`Change::Internal`] | `0.x.(z+1)` | `x.y.(z+1)` |
//!
//! Under `0.x`, additive and internal changes land in the same slot. That is not
//! an oversight: Cargo treats both as compatible for a `0.x` crate, so there is
//! no third slot to put them in. The [`Change`] is still reported, so the reason
//! for a release is never lost even when the number cannot express it.

use std::collections::BTreeSet;

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

/// Why a release is being cut. Derived from evidence, never from intent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    /// An observable contract was removed or redefined. Existing callers can
    /// break.
    Breaking,
    /// Observable surface was added and none was removed. Existing callers keep
    /// working.
    Additive,
    /// The observable surface is identical. Fixes and internals.
    Internal,
}

impl Change {
    /// The word an operator reads.
    pub fn as_str(self) -> &'static str {
        match self {
            Change::Breaking => "breaking",
            Change::Additive => "additive",
            Change::Internal => "internal",
        }
    }
}

/// Why the version was rejected. Bumping needs a numeric triple; ordering does
/// not, so these only ever come from the decide path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VersionError {
    NotCanonical(String),
    NotTriple(String),
    NotNumeric(String),
}

impl std::fmt::Display for VersionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            VersionError::NotCanonical(value) => write!(
                f,
                "{value:?} is not a canonical coordinate: expected a non-empty \
                 segment of alphanumerics, '.', '_' and '-', with no surrounding \
                 whitespace"
            ),
            VersionError::NotTriple(value) => write!(
                f,
                "{value:?} is not a major.minor.patch triple, so there is no slot \
                 to advance; name the next version explicitly"
            ),
            VersionError::NotNumeric(value) => write!(
                f,
                "{value:?} has a non-numeric slot, so advancing it would invent an \
                 ordering; name the next version explicitly"
            ),
        }
    }
}

impl std::error::Error for VersionError {}

/// A release version that can be advanced: exactly three numeric slots.
///
/// Versions that are merely *comparable* stay strings and go through
/// [`version_newer`]. This type exists only where a next version has to be
/// produced, because that is the only operation that needs the shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Version {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
}

impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let Self {
            major,
            minor,
            patch,
        } = self;
        write!(f, "{major}.{minor}.{patch}")
    }
}

impl Version {
    /// Parse `major.minor.patch`. Anything else is refused rather than coerced:
    /// guessing which slot of `147.0.7727.108-weles.1` means "minor" would
    /// produce a coordinate nobody chose.
    pub fn parse(value: &str) -> Result<Self, VersionError> {
        if !canonical_coordinate(value) {
            return Err(VersionError::NotCanonical(value.to_string()));
        }
        let mut parts = value.split('.');
        let (Some(major), Some(minor), Some(patch), None) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            return Err(VersionError::NotTriple(value.to_string()));
        };
        let (Ok(major), Ok(minor), Ok(patch)) = (
            major.parse::<u64>(),
            minor.parse::<u64>(),
            patch.parse::<u64>(),
        ) else {
            return Err(VersionError::NotNumeric(value.to_string()));
        };
        Ok(Self {
            major,
            minor,
            patch,
        })
    }

    /// True while the major slot is zero, when the minor slot carries the
    /// compatibility boundary.
    pub fn is_unstable(self) -> bool {
        self.major == u64::MIN
    }

    /// The version this change produces. See the table in the module docs.
    pub fn next(self, change: Change) -> Self {
        let step = u64::from(true);
        let zero = u64::MIN;
        match (change, self.is_unstable()) {
            (Change::Breaking, true) => Self {
                minor: self.minor.saturating_add(step),
                patch: zero,
                ..self
            },
            (Change::Breaking, false) => Self {
                major: self.major.saturating_add(step),
                minor: zero,
                patch: zero,
            },
            (Change::Additive, false) => Self {
                minor: self.minor.saturating_add(step),
                patch: zero,
                ..self
            },
            // Under 0.x an additive change is compatible, exactly like an
            // internal one, and the patch slot is the only one left.
            (Change::Additive, true) | (Change::Internal, _) => Self {
                patch: self.patch.saturating_add(step),
                ..self
            },
        }
    }
}

/// The observable command surface of one build.
///
/// The command list is the contract for these products: it is what a caller can
/// invoke, and it is what the July incident was reduced to counting when it
/// needed to tell two brokers apart. Comparing it is how a change gets
/// classified without asking anyone what they meant to do.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Surface {
    pub commands: BTreeSet<String>,
}

impl Surface {
    pub fn from_commands<I, S>(commands: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            commands: commands.into_iter().map(Into::into).collect(),
        }
    }

    /// Read a surface from whatever shape a product's `help` produces.
    ///
    /// Two shapes exist across this fleet and both are contracts. Products that
    /// emit `{"commands": [...]}` are read as JSON. Products built on clap print a
    /// `Commands:` section instead, which is how Stado itself describes its own
    /// surface — and until this read both shapes, the classifier could only be
    /// pointed at one product in a fleet of ten.
    ///
    /// A build that answers with neither gets an error naming what was expected,
    /// rather than an empty surface that would silently classify every release as
    /// internal.
    pub fn from_help(body: &str) -> Result<Self, String> {
        match Self::from_help_json(body) {
            Ok(surface) => Ok(surface),
            Err(json_error) => Self::from_clap_help(body).map_err(|clap_error| {
                format!("{json_error}; and not a clap command list either: {clap_error}")
            }),
        }
    }

    /// Parse the `Commands:` section clap prints, taking the first token of each
    /// indented line.
    ///
    /// `help` itself is skipped: clap injects it into every application, so it can
    /// never represent a change anyone made.
    pub fn from_clap_help(body: &str) -> Result<Self, String> {
        let mut names = BTreeSet::new();
        let mut inside = false;
        for line in body.lines() {
            if line.trim() == "Commands:" {
                inside = true;
                continue;
            }
            if !inside {
                continue;
            }
            // The section ends at the blank line before the next heading, and any
            // unindented line is that heading arriving early.
            if line.trim().is_empty() || !line.starts_with(char::is_whitespace) {
                break;
            }
            let Some(name) = line.split_whitespace().next() else {
                continue;
            };
            let name = name.trim_end_matches(',');
            if name == "help" {
                continue;
            }
            names.insert(name.to_string());
        }
        if names.is_empty() {
            return Err("no \"Commands:\" section with indented entries".to_string());
        }
        Ok(Self { commands: names })
    }

    /// Read a surface from a product's `help` output: `{"commands": [...]}`.
    pub fn from_help_json(body: &str) -> Result<Self, String> {
        let parsed: serde_json::Value =
            serde_json::from_str(body).map_err(|err| format!("help output is not JSON: {err}"))?;
        let commands = parsed
            .get("commands")
            .and_then(|value| value.as_array())
            .ok_or_else(|| "help output has no \"commands\" array".to_string())?;
        let mut names = BTreeSet::new();
        for entry in commands {
            let name = entry
                .as_str()
                .ok_or_else(|| "a \"commands\" entry is not a string".to_string())?;
            names.insert(name.to_string());
        }
        if names.is_empty() {
            return Err("help output lists no commands".to_string());
        }
        Ok(Self { commands: names })
    }

    /// What one surface gained and lost relative to another.
    pub fn diff(published: &Self, candidate: &Self) -> SurfaceDiff {
        SurfaceDiff {
            removed: published
                .commands
                .difference(&candidate.commands)
                .cloned()
                .collect(),
            added: candidate
                .commands
                .difference(&published.commands)
                .cloned()
                .collect(),
        }
    }
}

/// Commands gained and lost between two builds.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SurfaceDiff {
    pub added: Vec<String>,
    pub removed: Vec<String>,
}

impl SurfaceDiff {
    /// The rule, in one place:
    ///
    /// - anything removed, or a breaking change the operator declares because it
    ///   is not visible in the command list (a field dropped from a payload, a
    ///   stored format changed), is [`Change::Breaking`];
    /// - anything added and nothing removed is [`Change::Additive`];
    /// - an identical surface is [`Change::Internal`].
    ///
    /// `declared_breaking` can only ever escalate. There is deliberately no flag
    /// that lowers the classification: the evidence wins over the intent, so a
    /// removed command cannot be published as a patch by asserting that it is
    /// fine.
    pub fn classify(&self, declared_breaking: bool) -> Change {
        if declared_breaking || !self.removed.is_empty() {
            Change::Breaking
        } else if !self.added.is_empty() {
            Change::Additive
        } else {
            Change::Internal
        }
    }
}

/// The whole decision: what changed, and what to call it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decision {
    pub current: Version,
    pub next: Version,
    pub change: Change,
    pub diff: SurfaceDiff,
}

/// Decide the next version from the published surface, the candidate surface,
/// and whatever breakage the operator declares on top.
pub fn decide(
    current: Version,
    published: &Surface,
    candidate: &Surface,
    declared_breaking: bool,
) -> Decision {
    let diff = Surface::diff(published, candidate);
    let change = diff.classify(declared_breaking);
    Decision {
        current,
        next: current.next(change),
        change,
        diff,
    }
}
