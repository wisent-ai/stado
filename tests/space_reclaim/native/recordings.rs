//! At the disk-full rule: everything the fleet put on the host goes, the
//! user's data stays, and the Weles recordings cleaner sweeps the root the
//! host's registry entry declares (`targets[].weles.recordings_dir`, the one
//! the Weles launcher writes to) and nothing in its place when none is
//! declared.

use std::fs;
use std::path::Path;

use serde_json::json;

use super::Native;

impl Native {
    /// Declare `dir` as this host's Weles recordings root in its registry
    /// entry, through the registry command an operator uses.
    pub fn declare_weles_recordings(&self, dir: &Path) {
        let policy = json!({
            "enabled": false,
            "actions": [],
            "recordings_dir": dir.display().to_string(),
        });
        self.success(&[
            "registry",
            "set",
            "--path",
            "targets.0.weles",
            "--value",
            &policy.to_string(),
            "--json",
        ]);
    }
}

/// Write `body` at each of `paths`, creating their directories.
fn seed(paths: &[std::path::PathBuf], body: &[u8]) {
    for path in paths {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, body).unwrap();
    }
}

#[test]
fn at_the_threshold_everything_the_fleet_put_there_goes_and_user_data_stays() {
    let native = Native::new("at-threshold");
    let ssh_key = native.home.join(".ssh/id_ed25519");
    let notes = native.home.join("Documents/notes.txt");
    seed(std::slice::from_ref(&ssh_key), b"private key\n");
    seed(std::slice::from_ref(&notes), b"the user's own file\n");
    let caches: Vec<_> = ["repo-a", "repo-b", "repo-c"]
        .iter()
        .map(|repo| native.home.join(format!("work/{repo}/target")))
        .collect();
    for cache in &caches {
        native.cache(cache);
    }
    let logs: Vec<_> = ["request.json"]
        .iter()
        .map(|name| native.home.join(".omp/logs").join(name))
        .collect();
    seed(&logs, b"{}");
    // The recordings root this host's registry entry declares, as the Weles
    // launcher requires; the cleaner sweeps it and nothing in its place.
    let recordings_root = native.home.join("weles-recordings");
    native.declare_weles_recordings(&recordings_root);
    let recordings: Vec<_> = ["run-1"]
        .iter()
        .map(|run| recordings_root.join(run).join("video.mp4"))
        .collect();
    seed(&recordings, b"not uploaded anywhere");
    let user_data = native.fill_with_user_data();
    let host_snapshots_before = crate::host_snapshots();

    let report = native.cleanup();
    // The snapshot cleaner reads the isolated volume, which has none, and
    // leaves every snapshot of the machine's own volume where it was.
    assert_eq!(
        report["local_snapshot_volume"],
        json!(fs::canonicalize(&native.home).unwrap().display().to_string()),
        "{report}"
    );
    let host_snapshots_after = crate::host_snapshots();
    for snapshot in &host_snapshots_before {
        assert!(
            host_snapshots_after.contains(snapshot),
            "the pass deleted the machine's own snapshot {snapshot}: {report}"
        );
    }
    assert_eq!(report["rule"]["triggered"], true, "{report}");
    assert_eq!(report["pressure_active"], true);
    let cleaners = &report["cleaners"];
    assert_eq!(cleaners["build_caches"]["deleted_items"], json!(caches.len()), "{report}");
    assert_eq!(cleaners["agent_logs"]["deleted_items"], json!(logs.len()), "{report}");
    assert_eq!(cleaners["weles_recordings"]["deleted_items"], json!(recordings.len()), "{report}");
    for gone in caches.iter().chain(&logs).chain(&recordings) {
        assert!(!gone.exists(), "{} survived the rule", gone.display());
    }
    assert_eq!(fs::read(&ssh_key).unwrap(), b"private key\n");
    assert_eq!(fs::read(&notes).unwrap(), b"the user's own file\n");
    assert!(user_data.is_file(), "the user's data was deleted");
    // The volume is still full of the user's data, which the rule never takes.
    assert_eq!(report["outcome"], "still_full", "{report}");
}

#[test]
fn at_the_threshold_a_host_without_a_declared_recordings_root_keeps_every_recording() {
    let native = Native::new("weles-recordings-undeclared");
    let recordings: Vec<_> = ["run-1"]
        .iter()
        .map(|run| native.home.join("weles/recordings").join(run).join("video.mp4"))
        .collect();
    seed(&recordings, b"not uploaded anywhere");
    native.fill_with_user_data();

    let report = native.cleanup();
    assert_eq!(report["rule"]["triggered"], true, "{report}");
    let cleaner = &report["cleaners"]["weles_recordings"];
    // The one reason the cleaner gives is the missing declaration, and no
    // run directory was so much as scanned.
    assert!(
        cleaner["skipped"]
            .as_object()
            .is_some_and(|skipped| skipped.keys().eq(["recordings_dir_undeclared"])),
        "{report}"
    );
    assert_eq!(cleaner["deleted_items"], cleaner["scanned_items"], "{report}");
    for recording in &recordings {
        assert_eq!(
            fs::read(recording).unwrap(),
            b"not uploaded anywhere",
            "a recording outside any declared root was swept: {report}"
        );
    }
}
