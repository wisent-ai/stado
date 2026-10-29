//! The fleet's versioning rule, taken from the AutoVersion crate
//! (github.com/lbartoszcze/AutoVersion) instead of a copy kept here: given
//! the published version, the published surface and the candidate surface,
//! what kind of change this is and what the next version is. The
//! version-check workflow runs AutoVersion's fixtures against the crate Stado
//! builds with before it trusts a verdict from it.
//!
//! Also here: whether a declared SemVer is at least the version a change
//! requires, with SemVer 2.0.0 precedence for pre-release identifiers.

use std::cmp::Ordering;
use std::fmt;
use std::sync::LazyLock;

use regex::Regex;

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

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Change {
    Breaking,
    Additive,
    Internal,
}

impl From<autoversion::RuleError> for Refusal {
    fn from(error: autoversion::RuleError) -> Self {
        Self {
            name: error.refusal(),
            message: error.to_string(),
        }
    }
}

impl Change {
    pub(super) fn name(self) -> &'static str {
        match self {
            Self::Breaking => autoversion::BREAKING,
            Self::Additive => autoversion::ADDITIVE,
            Self::Internal => autoversion::INTERNAL,
        }
    }

    /// The class AutoVersion answered. A class this Stado does not know is
    /// refused by name, so a newer rule is never read as an older class.
    fn answered(name: &str) -> Result<Self, Refusal> {
        match name {
            autoversion::BREAKING => Ok(Self::Breaking),
            autoversion::ADDITIVE => Ok(Self::Additive),
            autoversion::INTERNAL => Ok(Self::Internal),
            other => Err(Refusal {
                name: "unknown-change",
                message: format!(
                    "AutoVersion answered the class {other:?}, which this Stado does not know"
                ),
            }),
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

/// The whole answer, AutoVersion's. A declared break may only escalate the
/// class.
pub(super) fn decide(
    current: &str,
    published: &[String],
    candidate: &[String],
    declared_breaking: bool,
) -> Result<Decision, Refusal> {
    let version = autoversion::Version::parse(current)?;
    let (change, removed, added) = autoversion::classify(published, candidate, declared_breaking)?;
    Ok(Decision {
        current: version.to_string(),
        change: Change::answered(change)?,
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

/// The least version the rule lets a release of new source take after
/// `current`: the step of an internal change. A change to the product's
/// surface takes more, as `decide` answers for that surface; no release of
/// new source keeps `current`, because a published version attests its
/// source for good.
pub(crate) fn least_next(current: &str) -> Result<String, String> {
    autoversion::Version::parse(current)
        .map(|version| version.advance(autoversion::INTERNAL).to_string())
        .map_err(|refusal| Refusal::from(refusal).to_string())
}
