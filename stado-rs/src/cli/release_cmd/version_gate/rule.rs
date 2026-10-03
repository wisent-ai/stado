//! The fleet's versioning rule (github.com/lbartoszcze/AutoVersion, SPEC.md at
//! v0.1.0): given the published version, the published surface and the
//! candidate surface, what kind of change this is and what the next version
//! is. The rule's SPEC keeps one small implementation per consumer, held
//! identical by its shared fixtures; the version-check workflow runs those
//! fixtures against this port before it trusts a verdict from it.
//!
//! Also here: whether a declared SemVer is at least the version a change
//! requires, with SemVer 2.0.0 precedence for pre-release identifiers.

use std::cmp::Ordering;
use std::collections::BTreeSet;
use std::fmt;
use std::sync::LazyLock;

use regex::Regex;

/// The value a version slot resets to when a higher slot advances, and the
/// value of an unstable major (SPEC.md, "What moves").
const SLOT_RESET: u64 = 0;
/// How far one change advances one slot (SPEC.md, "What moves").
const SLOT_STEP: u64 = 1;

/// A refusal, named the way the rule's fixtures name it.
#[derive(Debug)]
pub(super) struct Refusal {
    pub(super) name: &'static str,
    pub(super) message: String,
}

impl fmt::Display for Refusal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "refused ({}): {}", self.name, self.message)
    }
}

fn refuse(name: &'static str, message: String) -> Refusal {
    Refusal { name, message }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Change {
    Breaking,
    Additive,
    Internal,
}

impl Change {
    pub(super) fn name(self) -> &'static str {
        match self {
            Self::Breaking => "breaking",
            Self::Additive => "additive",
            Self::Internal => "internal",
        }
    }
}

struct Version {
    major: u64,
    minor: u64,
    patch: u64,
}

impl fmt::Display for Version {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

/// A segment that survives a URL path and a filesystem key unchanged.
fn canonical(value: &str) -> bool {
    !value.is_empty()
        && value.trim() == value
        && value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "._-".contains(character))
}

impl Version {
    fn parse(value: &str) -> Result<Self, Refusal> {
        if !canonical(value) {
            return Err(refuse(
                "not-canonical",
                format!("{value:?} is not a canonical coordinate: expected a non-empty segment of alphanumerics, '.', '_' and '-', with no surrounding whitespace"),
            ));
        }
        let slots = value.split('.').collect::<Vec<_>>();
        let [major, minor, patch] = slots.as_slice() else {
            return Err(refuse(
                "not-a-triple",
                format!("{value:?} is not a major.minor.patch triple, so there is no slot to advance; name the next version explicitly"),
            ));
        };
        let numeric = |slot: &str| {
            (!slot.is_empty() && slot.bytes().all(|byte| byte.is_ascii_digit()))
                .then(|| slot.parse::<u64>().ok())
                .flatten()
        };
        match (numeric(major), numeric(minor), numeric(patch)) {
            (Some(major), Some(minor), Some(patch)) => Ok(Self { major, minor, patch }),
            _ => Err(refuse(
                "not-numeric",
                format!("{value:?} has a non-numeric slot, so advancing it would invent an ordering; name the next version explicitly"),
            )),
        }
    }

    /// The version this change produces. While the major slot is zero, the
    /// minor slot carries compatibility.
    fn advance(&self, change: Change) -> Self {
        let (major, minor, patch) = match (change, self.major == SLOT_RESET) {
            (Change::Breaking, true) => (self.major, self.minor + SLOT_STEP, SLOT_RESET),
            (Change::Breaking, false) => (self.major + SLOT_STEP, SLOT_RESET, SLOT_RESET),
            (Change::Additive, false) => (self.major, self.minor + SLOT_STEP, SLOT_RESET),
            _ => (self.major, self.minor, self.patch + SLOT_STEP),
        };
        Self {
            major,
            minor,
            patch,
        }
    }
}

pub(super) struct Decision {
    pub(super) current: String,
    pub(super) change: Change,
    pub(super) next: String,
    pub(super) removed: Vec<String>,
    pub(super) added: Vec<String>,
}

fn surface(names: &[String], side: &str) -> Result<BTreeSet<String>, Refusal> {
    let collected = names.iter().cloned().collect::<BTreeSet<_>>();
    if collected.is_empty() {
        return Err(refuse(
            "empty-surface",
            format!("the {side} surface is empty, which is far more likely to be a broken extractor than a product that promises nothing"),
        ));
    }
    Ok(collected)
}

/// The whole answer. A declared break may only escalate the class.
pub(super) fn decide(
    current: &str,
    published: &[String],
    candidate: &[String],
    declared_breaking: bool,
) -> Result<Decision, Refusal> {
    let version = Version::parse(current)?;
    let before = surface(published, "published")?;
    let after = surface(candidate, "candidate")?;
    let removed = before.difference(&after).cloned().collect::<Vec<_>>();
    let added = after.difference(&before).cloned().collect::<Vec<_>>();
    let change = if declared_breaking || !removed.is_empty() {
        Change::Breaking
    } else if !added.is_empty() {
        Change::Additive
    } else {
        Change::Internal
    };
    Ok(Decision {
        current: version.to_string(),
        change,
        next: version.advance(change).to_string(),
        removed,
        added,
    })
}

static SEMVER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-([0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*))?(?:\+[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?$",
    )
    .expect("static")
});

type Semver = ([String; 3], Option<Vec<String>>);

fn semver(value: &str) -> Result<Semver, String> {
    let captures = SEMVER
        .captures(value)
        .ok_or_else(|| format!("invalid semantic version: {value}"))?;
    let core = [&captures[1], &captures[2], &captures[3]].map(str::to_string);
    let prerelease = captures
        .get(4)
        .map(|found| found.as_str().split('.').map(str::to_string).collect());
    Ok((core, prerelease))
}

/// Two unsigned decimal strings compared as numbers, of any length.
fn numeric_order(left: &str, right: &str) -> Ordering {
    let (left, right) = (left.trim_start_matches('0'), right.trim_start_matches('0'));
    left.len().cmp(&right.len()).then_with(|| left.cmp(right))
}

fn prerelease_order(left: &Option<Vec<String>>, right: &Option<Vec<String>>) -> Ordering {
    let (Some(left), Some(right)) = (left, right) else {
        // A release outranks any pre-release of the same core.
        return right.is_some().cmp(&left.is_some());
    };
    let digits = |text: &str| text.bytes().all(|byte| byte.is_ascii_digit());
    for (a, b) in left.iter().zip(right) {
        if a == b {
            continue;
        }
        return match (digits(a), digits(b)) {
            (true, true) => numeric_order(a, b),
            (true, false) => Ordering::Less,
            (false, true) => Ordering::Greater,
            (false, false) => a.cmp(b),
        };
    }
    left.len().cmp(&right.len())
}

/// How two valid SemVer strings order, by core numbers and then pre-release.
pub(crate) fn semver_order(left: &str, right: &str) -> Result<Ordering, String> {
    let (left_core, left_pre) = semver(left)?;
    let (right_core, right_pre) = semver(right)?;
    Ok(left_core
        .iter()
        .zip(&right_core)
        .map(|(a, b)| numeric_order(a, b))
        .find(|order| *order != Ordering::Equal)
        .unwrap_or_else(|| prerelease_order(&left_pre, &right_pre)))
}

/// Whether ACTUAL is a valid SemVer at least MINIMUM.
pub(super) fn semver_at_least(actual: &str, minimum: &str) -> Result<bool, String> {
    Ok(semver_order(actual, minimum)? != Ordering::Less)
}
