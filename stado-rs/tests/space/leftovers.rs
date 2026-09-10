//! What a release install leaves on a host, reclaimed for real inside the
//! fixture's own home.
//!
//! `stado release install-local` writes an attestation copy and a retained
//! archive under `~/.stado/releases/<product>/<version>/<platform>/`, the
//! installed coordinate in `~/.stado/bin/<product>.release-version`, and one
//! dated backup of the replaced binary beside it. The `delivery_leftovers`
//! stage keeps the installed version, the newest version and the newest
//! backup, and takes the stale rest.

use std::fs::{self, File, FileTimes};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use crate::fixture::{only_stage, reported_paths, Host, AUDIT_LOG, TARGET};

/// Old enough for the stage's age gate, which refuses anything younger than
/// a day.
const AGED_DAYS: u64 = 3;

/// A delivered version's tree, as install-local leaves it, aged past the gate.
fn deliver(host: &Host, version: &str) -> PathBuf {
    let tree = host.under_home(&format!(".stado/releases/stado/{version}"));
    let platform = tree.join("darwin-arm64");
    fs::create_dir_all(&platform).expect("create the delivered version tree");
    fs::write(platform.join("stado"), b"attestation copy\n").expect("write the attestation copy");
    fs::write(
        platform.join("stado-reader-convergence.tar.gz"),
        b"retained archive\n",
    )
    .expect("write the retained archive");
    age(&tree);
    tree
}

/// A dated backup of the installed binary, aged past the gate.
fn backup(host: &Host, stamp: &str) -> PathBuf {
    let path = host.under_home(&format!(".stado/bin/stado.release-backup-{stamp}"));
    fs::write(&path, format!("binary before {stamp}\n")).expect("write the dated backup");
    age(&path);
    path
}

fn age(path: &Path) {
    let aged = SystemTime::now() - Duration::from_secs(AGED_DAYS * 24 * 60 * 60);
    File::open(path)
        .expect("open the leftover to age it")
        .set_times(FileTimes::new().set_accessed(aged).set_modified(aged))
        .expect("age the leftover past the gate");
}

fn assert_inside(root: &Path, paths: &[String]) {
    let root = root.to_string_lossy().to_string();
    for path in paths {
        assert!(
            path.starts_with(&root),
            "the preview named {path}, which is outside this test's tempdir {root}; refusing to apply"
        );
    }
}

#[test]
fn the_installed_and_newest_versions_and_the_newest_backup_survive_the_rest_is_taken() {
    let host = Host::new();
    let stale = deliver(&host, "0.1.0");
    let installed = deliver(&host, "0.2.0");
    let newest = deliver(&host, "0.3.0");
    // The host runs 0.2.0: a newer delivery was rolled back from, so the
    // installed coordinate names an older version than the newest tree.
    fs::write(
        host.under_home(".stado/bin/stado.release-version"),
        "0.2.0\n",
    )
    .expect("write the installed coordinate");
    let old_backup = backup(&host, "20260801");
    let newest_backup = backup(&host, "20260901");
    let live = host.under_home(".stado/bin/stado");
    assert!(live.is_file(), "the fixture installs the real binary");

    let preview = host.json(&[
        "space",
        "reclaim",
        TARGET,
        "--stage",
        "delivery_leftovers",
        "--dry-run",
        "--json",
    ]);
    let stage = only_stage(&preview, "delivery_leftovers");
    let mut paths = reported_paths(stage);
    assert_inside(&host.root, &paths);
    paths.sort();
    let mut expected = vec![
        stale.to_string_lossy().to_string(),
        old_backup.to_string_lossy().to_string(),
    ];
    expected.sort();
    assert_eq!(
        paths, expected,
        "the preview named the wrong leftovers: {stage}"
    );
    assert!(stale.exists(), "a preview removed a version tree");
    assert!(old_backup.exists(), "a preview removed a backup");

    let reason = "space area: proving delivery leftovers keep the installed version";
    let report = host.json(&[
        "space",
        "reclaim",
        TARGET,
        "--stage",
        "delivery_leftovers",
        "--apply",
        "--reason",
        reason,
        "--json",
    ]);
    let stage = only_stage(&report, "delivery_leftovers");
    assert_eq!(stage["items"].as_u64(), Some(2));
    assert!(!stale.exists(), "the stale version tree survived");
    assert!(!old_backup.exists(), "the older backup survived");
    assert!(
        installed.join("darwin-arm64/stado").is_file(),
        "the installed version's attestation copy was taken"
    );
    assert!(
        newest
            .join("darwin-arm64/stado-reader-convergence.tar.gz")
            .is_file(),
        "the newest version's retained archive was taken"
    );
    assert!(newest_backup.is_file(), "the newest backup was taken");
    assert!(live.is_file(), "the live binary was taken");
    assert!(
        host.under_home(".stado/bin/stado.release-version")
            .is_file(),
        "the installed coordinate was taken"
    );
    let audit = fs::read_to_string(host.under_home(AUDIT_LOG))
        .expect("the applied run recorded itself here");
    let record: serde_json::Value =
        serde_json::from_str(audit.trim()).expect("the audit log is one JSON-lines record");
    assert_eq!(record["reason"], reason);
    assert_eq!(record["stages"][0]["stage"], "delivery_leftovers");
}

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
