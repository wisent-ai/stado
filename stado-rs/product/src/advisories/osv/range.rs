//! One OSV range's events, and whether a locked version lies inside it, as
//! the schema's evaluation walks the events in version order
//! (https://ossf.github.io/osv-schema/#evaluation).

use anyhow::{Context, Result};
use semver::{BuildMetadata, Comparator, Version};
use serde::Deserialize;

/// The lowest semver version, which precedes every release and pre-release.
/// OSV writes its major alone for "every version from the first".
const FIRST_VERSION: &str = "0.0.0-0";

#[derive(Deserialize)]
pub(super) struct Event {
    introduced: Option<String>,
    fixed: Option<String>,
    last_affected: Option<String>,
}

/// What one event of a range says from its version on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Mark {
    Introduced,
    Fixed,
    LastAffected,
}

/// An event's version. OSV writes the first version's major alone for "every
/// version from the first", which is read as the lowest semver version. An
/// advisory may write a crate version without its minor or patch (`0.62`), as
/// a Cargo requirement does; that is the first version the requirement names,
/// its missing parts those of the lowest version.
fn event_version(advisory: &str, written: &str) -> Result<Version> {
    let first = Version::parse(FIRST_VERSION)
        .with_context(|| format!("{FIRST_VERSION} is not a semver version"))?;
    if written == first.major.to_string() {
        return Ok(first);
    }
    if let Ok(version) = Version::parse(written) {
        return Ok(version);
    }
    let comparator = Comparator::parse(&format!("={written}")).with_context(|| {
        format!("advisory {advisory} names the version {written:?}, which is not semver")
    })?;
    let part = |named: Option<u64>, lowest: u64| match named {
        Some(part) => part,
        None => lowest,
    };
    Ok(Version {
        major: comparator.major,
        minor: part(comparator.minor, first.minor),
        patch: part(comparator.patch, first.patch),
        pre: comparator.pre,
        build: BuildMetadata::EMPTY,
    })
}

/// Whether `locked` lies in the range `events` describe, and the fixed
/// versions that close it.
pub(super) fn evaluate(
    advisory: &str,
    events: &[Event],
    locked: &Version,
) -> Result<(bool, Vec<Version>)> {
    let mut ordered = Vec::new();
    for event in events {
        for (mark, written) in [
            (Mark::Introduced, &event.introduced),
            (Mark::Fixed, &event.fixed),
            (Mark::LastAffected, &event.last_affected),
        ] {
            if let Some(written) = written {
                ordered.push((event_version(advisory, written)?, mark));
            }
        }
    }
    ordered.sort();
    let mut inside = false;
    let mut fixed = Vec::new();
    for (version, mark) in ordered {
        match mark {
            Mark::Introduced => inside |= *locked >= version,
            Mark::Fixed => {
                inside &= *locked < version;
                fixed.push(version);
            }
            Mark::LastAffected => inside &= *locked <= version,
        }
    }
    Ok((inside, fixed))
}
