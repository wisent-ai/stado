//! Direction-aware ordering of two exact semantic versions.

use std::cmp::Ordering;

/// Direction-aware ordering of two exact semantic versions.
///
/// The numeric core decides, per semver; equal cores are settled by the
/// prerelease the same way: a release outranks its own prereleases, numeric
/// identifiers order numerically and below alphanumeric ones, alphanumeric
/// ones lexically, and a longer list outranks its own prefix. Both inputs
/// here have already passed [`host_release::is_exact_semver`], so the parse
/// cannot fail; the `Option` is the parse's own honesty, not a third answer.
pub(in crate::cli::service_converge) fn version_order(left: &str, right: &str) -> Option<Ordering> {
    fn core(version: &str) -> Option<(u64, u64, u64)> {
        let core = version.split_once('-').map_or(version, |(core, _)| core);
        let mut parts = core.split('.');
        let triple = (
            parts.next()?.parse().ok()?,
            parts.next()?.parse().ok()?,
            parts.next()?.parse().ok()?,
        );
        if parts.next().is_some() {
            return None;
        }
        Some(triple)
    }
    fn prerelease(version: &str) -> &str {
        version
            .split_once('-')
            .map_or("", |(_, prerelease)| prerelease)
    }
    fn prerelease_order(left: &str, right: &str) -> Ordering {
        match (left.is_empty(), right.is_empty()) {
            (true, true) => return Ordering::Equal,
            // A release outranks every one of its own prereleases.
            (true, false) => return Ordering::Greater,
            (false, true) => return Ordering::Less,
            (false, false) => {}
        }
        let mut lefts = left.split('.');
        let mut rights = right.split('.');
        loop {
            let ordering = match (lefts.next(), rights.next()) {
                (None, None) => return Ordering::Equal,
                (None, Some(_)) => return Ordering::Less,
                (Some(_), None) => return Ordering::Greater,
                (Some(left), Some(right)) => {
                    let numeric = |identifier: &str| identifier.bytes().all(|b| b.is_ascii_digit());
                    match (numeric(left), numeric(right)) {
                        (true, true) => left
                            .parse::<u64>()
                            .unwrap_or(u64::MAX)
                            .cmp(&right.parse::<u64>().unwrap_or(u64::MAX)),
                        // Numeric identifiers order below alphanumeric ones.
                        (true, false) => Ordering::Less,
                        (false, true) => Ordering::Greater,
                        (false, false) => left.cmp(right),
                    }
                }
            };
            if ordering != Ordering::Equal {
                return ordering;
            }
        }
    }
    let ordering = core(left)?.cmp(&core(right)?);
    if ordering != Ordering::Equal {
        return Some(ordering);
    }
    Some(prerelease_order(prerelease(left), prerelease(right)))
}
