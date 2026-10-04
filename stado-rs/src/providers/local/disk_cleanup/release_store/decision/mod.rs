//! The keep-or-reclaim decision for one product, and the version ordering it
//! rests on.

use std::collections::{BTreeMap, BTreeSet};

/// Version strings ordered as numbers, newest last; a version that is not
/// dotted numbers sorts before every one that is, so it is never counted as
/// the newest of anything.
fn version_key(version: &str) -> (bool, Vec<u64>) {
    let parts: Option<Vec<u64>> = version
        .split('.')
        .map(|part| part.parse::<u64>().ok())
        .collect();
    match parts {
        Some(numbers) => (true, numbers),
        None => (false, Vec::new()),
    }
}

/// Why one version survives a pass, or `None` when nothing needs it.
///
/// The reason strings are the report's own skip keys, so the retention
/// decision and what an operator reads are one thing rather than two that can
/// drift apart.
pub(crate) type KeepReason = Option<&'static str>;

/// The retention decision for one product, from names alone.
///
/// A version survives only when something on this host or in the fleet still
/// uses it: this host's installed state names it, the registry declares it,
/// the configuration pins it, a release run in flight names it, or it is the
/// newest release a host can install from its family. `versions` is every
/// version directory found, in any order; `complete` maps a version to the
/// release families it holds completely, as
/// [`complete_families`](super::inventory::families::complete_families) read
/// them off the disk.
pub(crate) fn retention_decision<'a>(
    versions: &'a [String],
    complete: &BTreeMap<String, BTreeSet<&'static str>>,
    by_host: &BTreeSet<String>,
    by_declaration: &BTreeSet<String>,
    by_config: &BTreeSet<String>,
    by_run: &BTreeSet<String>,
) -> Vec<(&'a str, KeepReason)> {
    let mut ordered: Vec<&'a str> = versions.iter().map(String::as_str).collect();
    ordered.sort_by_key(|version| version_key(version));
    // The newest version that is genuinely deployable, PER FAMILY: what a
    // host joining the fleet installs. Per family and not once overall,
    // because a product may publish both and a host reads one: `stado` has an
    // installer family and a signed one, and keeping only the newest complete
    // release of either would leave the other installer with nothing.
    let mut newest_complete: BTreeMap<&'static str, &str> = BTreeMap::new();
    for version in ordered.iter().rev().copied() {
        for family in complete.get(version).into_iter().flatten() {
            newest_complete.entry(family).or_insert(version);
        }
    }
    ordered
        .into_iter()
        .map(|version| {
            let reason = if by_host.contains(version) {
                Some("host_state_names_it")
            } else if by_declaration.contains(version) {
                Some("host_declares_it")
            } else if by_config.contains(version) {
                Some("config_pins_it")
            } else if by_run.contains(version) {
                Some("pipeline_run_names_it")
            } else {
                // Reported by family, so an operator reading the reason knows
                // which installer still needs this version.
                newest_complete
                    .iter()
                    .find(|(_, newest)| **newest == version)
                    .map(|(family, _)| *family)
            };
            (version, reason)
        })
        .collect()
}
