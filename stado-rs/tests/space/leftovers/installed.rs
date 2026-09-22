//! What the stage keeps when the host says which version is installed: that
//! version, the newest one, and the newest backup.

use std::fs;

use crate::fixture::{only_stage, reported_paths, Host, AUDIT_LOG, TARGET};

use super::{assert_inside, backup, deliver};

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
fn recognised_copies_keep_the_newest_and_refuse_unknown_shapes() {
    let host = Host::new();
    let live = host.under_home(".stado/bin/stado");
    let old = [
        "release-backup-20260801",
        "0.7.0-backup-20260810",
        "bak-20260811",
        "pre-converge",
    ]
    .map(|suffix| host.under_home(&format!(".stado/bin/stado.{suffix}")));
    let previous = host.under_home(".stado/bin/stado.previous");
    let unknown = host.under_home(".stado/bin/stado.fleet-during-verify-20260818");
    for path in old.iter().chain([&previous, &unknown]) {
        fs::write(path, b"replaced binary\n").expect("write binary copy");
        fs::set_permissions(path, fs::metadata(&live).unwrap().permissions()).unwrap();
        age(path);
    }
    let newest = backup(&host, "20260701"); // Newest by mtime, not stamp; past the age gate.
    File::open(&newest)
        .unwrap()
        .set_times(
            FileTimes::new().set_modified(SystemTime::now() - Duration::from_secs(2 * 86400)),
        )
        .unwrap();
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
    let mut expected: Vec<_> = old
        .iter()
        .map(|path| path.to_string_lossy().to_string())
        .collect();
    expected.sort();
    assert_eq!(paths, expected, "{stage}");
    assert_eq!(
        stage["refused"],
        serde_json::json!([format!(
            "{}: unrecognised binary copy; retained",
            unknown.display()
        )])
    );
    assert!(
        old.iter().all(|path| path.is_file()),
        "preview removed a copy"
    );
    let report = host.json(&[
        "space",
        "reclaim",
        TARGET,
        "--stage",
        "delivery_leftovers",
        "--apply",
        "--reason",
        "space area: binary copy shapes",
        "--json",
    ]);
    assert_eq!(
        only_stage(&report, "delivery_leftovers")["items"],
        old.len()
    );
    assert!(
        old.iter().all(|path| !path.exists())
            && [live, newest, previous, unknown]
                .iter()
                .all(|path| path.is_file())
    );
}
