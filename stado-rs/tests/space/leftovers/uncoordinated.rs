//! What the stage keeps when nothing says which version is installed: the
//! newest, and any version whose copy is the installed binary byte for byte.
//! A delivery younger than the age gate is never taken, pinned or not.

use std::fs;

use crate::fixture::{only_stage, reported_paths, Host, TARGET};

use super::{age, assert_inside, deliver};

#[test]
fn a_host_with_no_installed_coordinate_keeps_only_its_newest_version() {
    let host = Host::new();
    let stale = deliver(&host, "0.1.0");
    let older = deliver(&host, "0.2.0");
    let newest = deliver(&host, "0.3.0");
    let report = host.json(&[
        "space",
        "reclaim",
        TARGET,
        "--stage",
        "delivery_leftovers",
        "--apply",
        "--reason",
        "space area: no installed coordinate pins nothing",
        "--json",
    ]);
    let stage = only_stage(&report, "delivery_leftovers");
    let mut paths = reported_paths(stage);
    assert_inside(&host.root, &paths);
    paths.sort();
    let mut expected = vec![
        stale.to_string_lossy().to_string(),
        older.to_string_lossy().to_string(),
    ];
    expected.sort();
    assert_eq!(paths, expected);
    assert!(!stale.exists() && !older.exists());
    assert!(newest.join("darwin-arm64/stado").is_file());
}

/// A product delivered by the path that writes no coordinate is pinned by
/// what it IS: the version whose attestation copy matches the installed
/// binary byte for byte survives, stale and superseded or not.
#[test]
fn the_version_whose_copy_is_the_installed_binary_survives_without_a_coordinate() {
    let host = Host::new();
    let stale = deliver(&host, "0.1.0");
    let identical = deliver(&host, "0.2.0");
    fs::remove_file(identical.join("darwin-arm64/stado")).unwrap();
    fs::hard_link(
        host.under_home(".stado/bin/stado"),
        identical.join("darwin-arm64/stado"),
    )
    .expect("stage the installed binary as 0.2.0's attestation copy");
    age(&identical);
    let newest = deliver(&host, "0.3.0");
    let report = host.json(&[
        "space",
        "reclaim",
        TARGET,
        "--stage",
        "delivery_leftovers",
        "--apply",
        "--reason",
        "space area: byte identity pins the installed version",
        "--json",
    ]);
    let stage = only_stage(&report, "delivery_leftovers");
    assert_eq!(
        reported_paths(stage),
        vec![stale.to_string_lossy().to_string()],
        "{stage}"
    );
    assert!(
        identical.join("darwin-arm64/stado").is_file(),
        "the attestation copy of the installed binary was taken"
    );
    assert!(newest.exists());
}

/// The newest delivery is the most recent one, not the highest version: a
/// rollback re-delivers an older version and that delivery is the one kept.
/// A delivery younger than the age gate survives even when it is neither
/// pinned nor newest.
#[test]
fn a_fresh_delivery_is_never_taken() {
    let host = Host::new();
    let pinned = deliver(&host, "0.1.0");
    fs::write(
        host.under_home(".stado/bin/stado.release-version"),
        "0.1.0\n",
    )
    .expect("write the installed coordinate");
    let superseded = deliver(&host, "0.3.0");
    let today = |version: &str| {
        let platform = host.under_home(&format!(".stado/releases/stado/{version}/darwin-arm64"));
        fs::create_dir_all(&platform).expect("create today's delivery");
        fs::write(platform.join("stado"), b"delivered today\n")
            .expect("write today's attestation copy");
        platform
    };
    let young = today("0.2.0");
    let newest = today("0.4.0");
    let report = host.json(&[
        "space",
        "reclaim",
        TARGET,
        "--stage",
        "delivery_leftovers",
        "--apply",
        "--reason",
        "space area: the age gate protects today's deliveries",
        "--json",
    ]);
    let stage = only_stage(&report, "delivery_leftovers");
    assert_eq!(
        reported_paths(stage),
        vec![superseded.to_string_lossy().to_string()],
        "{stage}"
    );
    assert!(
        pinned.join("darwin-arm64/stado").is_file(),
        "the installed version was taken"
    );
    assert!(
        young.join("stado").is_file(),
        "today's older delivery was taken"
    );
    assert!(
        newest.join("stado").is_file(),
        "today's newest delivery was taken"
    );
    assert!(
        !superseded.exists(),
        "the stale superseded delivery survived"
    );
}
