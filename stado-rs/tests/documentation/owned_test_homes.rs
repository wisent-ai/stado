//! A test may not reach the operator's home.
//!
//! The product derives real state from `HOME`: `stado status` records
//! `~/.stado/cache/registry-last-good.json`, `resolver serve` publishes
//! `~/.stado/resolver-state.json`, `scratch create` puts a lease's storage
//! root under it. A test that spawns the built binary without saying what
//! `HOME` is writes all of that into the operator's own home — on
//! 2026-09-08 eight areas did, and the operator's cached registry had been
//! replaced with a three-host fixture document by whichever area ran last.
//!
//! One route into that cache has since narrowed: `c637b026` stopped a registry
//! read from a Local-adapter store recording or reading the last-known-good
//! copy. The rule is not narrower for it. `HOME` still decides where the
//! resolver publishes, where a lease's storage root goes, which managed binary
//! `release host-state` reads and where a non-local read records its cache, and
//! a test that leaves the choice to whoever ran it has no answer for any of
//! them.
//!
//! The rule this file measures is the whole repair: an area that spawns the
//! product passes `HOME`. Judged per area directory rather than per file,
//! because a fixture module commonly owns the spawn for every case in its
//! area, and the case files then name neither the binary nor the override.
//!
//! It reads this revision's own tracked sources through
//! [`crate::source_files`] and touches no operator state.

use std::collections::{BTreeMap, BTreeSet};

/// Where the test areas live, relative to the repository root.
const AREA_ROOT: &str = "stado-rs/tests/";

/// The override an area has to pass. Any `HOME` a test owns is spelled this
/// way; the value cannot be judged from the source, which is why the
/// [`crate::source_files`] reading stops at the key.
const HOME_OVERRIDE: &str = "env(\"HOME\"";

/// Below this, the tracked-source listing is broken rather than clean, and a
/// silent pass would be the worst answer this file could give. The tree
/// carried well over fifty spawning areas when the rule was written.
const FEWEST_CREDIBLE_AREAS: usize = 30;

/// The environment variable naming the built binary, assembled here rather
/// than written out: this file has to carry the marker in order to search for
/// it, and a literal would make the search report itself.
fn spawn_marker() -> String {
    format!("CARGO_BIN_EXE{}", "_stado")
}

/// The area a tracked path belongs to: the first component under the tests
/// root, which is the directory a `[[test]]` target is built from.
fn area_of(path: &str) -> Option<(&str, &str)> {
    let rest = path.strip_prefix(AREA_ROOT)?;
    let area = rest.split('/').next()?;
    (!area.is_empty()).then_some((area, rest))
}

#[test]
fn a_test_area_that_spawns_the_product_passes_the_home_it_runs_it_with() {
    let root = crate::repository_root();
    let marker = spawn_marker();

    // Per area: where it spawns the product, and whether anything in it says
    // what HOME the child gets.
    let mut spawns: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut owns_home: BTreeSet<String> = BTreeSet::new();
    let mut areas: BTreeSet<String> = BTreeSet::new();

    for path in crate::source_files() {
        let Some((area, _)) = area_of(&path) else {
            continue;
        };
        let Ok(text) = std::fs::read_to_string(root.join(&path)) else {
            continue;
        };
        areas.insert(area.to_string());
        if text.contains(HOME_OVERRIDE) {
            owns_home.insert(area.to_string());
        }
        if let Some(line) = text.lines().position(|line| line.contains(&marker)) {
            spawns
                .entry(area.to_string())
                .or_default()
                .push(format!("{path}:{}", line + 1));
        }
    }

    assert!(
        areas.len() >= FEWEST_CREDIBLE_AREAS,
        "only {} test areas were read under {AREA_ROOT}; the tracked-source listing is broken, \
         so this check could not have judged anything",
        areas.len()
    );

    let offenders: Vec<String> = spawns
        .iter()
        .filter(|(area, _)| !owns_home.contains(*area))
        .map(|(area, sites)| format!("  {area}: {}", sites.join(", ")))
        .collect();

    assert!(
        offenders.is_empty(),
        "these test areas spawn the built product without saying what HOME it runs with, so \
         every case in them writes the operator's own ~/.stado — the registry cache, the \
         resolver state, a lease's storage root:\n{}\n\nAdd `.env(\"HOME\", <a directory this \
         test owns>)` where the area builds its command, next to WC_STORAGE_BACKEND, \
         WC_LOCAL_STORAGE_PATH and STADO_CONFIG, so a new case inherits it. A journey that \
         needs something out of the operator's home copies that input into its own tempdir — \
         stado-rs/tests/support/owned_home.rs does exactly that for the ssh identity and the \
         Stado configuration — rather than pointing HOME at the operator. Judged per area \
         directory, so the file listed above is where the spawn is, and the override may land \
         in any file of that area: a fixture module that owns the area's spawn is the usual \
         home for it.",
        offenders.join("\n")
    );
}
