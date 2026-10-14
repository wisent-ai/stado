//! The keep-or-reclaim decision for one product, and the version ordering it
//! rests on.

use std::collections::BTreeSet;

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
/// newest complete signed release, the one a host joining the fleet installs.
/// `versions` is every version directory found, in any order; `complete` is
/// the versions holding a complete signed release, as
/// [`signed_release_complete`](super::inventory::signed::signed_release_complete)
/// read them off the disk.
pub(crate) fn retention_decision<'a>(
    versions: &'a [String],
    complete: &BTreeSet<String>,
    by_host: &BTreeSet<String>,
    by_declaration: &BTreeSet<String>,
    by_config: &BTreeSet<String>,
    by_run: &BTreeSet<String>,
) -> Vec<(&'a str, KeepReason)> {
    let mut ordered: Vec<&'a str> = versions.iter().map(String::as_str).collect();
    ordered.sort_by_key(|version| version_key(version));
    let newest_complete = ordered
        .iter()
        .rev()
        .copied()
        .find(|version| complete.contains(*version));
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
            } else if newest_complete == Some(version) {
                Some("newest_signed_release_kept")
            } else {
                None
            };
            (version, reason)
        })
        .collect()
}
