//! `space reclaim`'s `delivery_leftovers` stage keeps the release the host
//! can roll back to: the version whose staged copy is `<product>.previous`
//! survives beside the installed and newest ones, so `release local restore`
//! still finds its archive after the installed release breaks.

use std::fs::{self, File, FileTimes};
use std::time::SystemTime;

use serde_json::Value;

use crate::native::Native;

const PLATFORM: &str = "darwin-arm64";

fn reclaim(native: &Native, mode: &str) -> Value {
    let mut args = vec![
        "space",
        "reclaim",
        "example-cleanup-host",
        "--stage",
        "delivery_leftovers",
        mode,
    ];
    if mode == "--apply" {
        args.extend(["--reason", "rollback target journey"]);
    }
    args.push("--json");
    let report = native.json(&args);
    native.observe(mode, report.clone());
    report
}

/// One delivered version of stado whose staged binary carries `bytes`,
/// its directory last modified at `modified`.
fn deliver(native: &Native, version: &str, bytes: &[u8], modified: SystemTime) {
    let root = native.home.join(".stado/releases/stado").join(version);
    let platform = root.join(PLATFORM);
    fs::create_dir_all(&platform).unwrap();
    fs::write(platform.join("stado"), bytes).unwrap();
    let times = FileTimes::new().set_modified(modified).set_accessed(modified);
    File::open(&root).unwrap().set_times(times).unwrap();
}

#[test]
fn the_rollback_targets_release_survives_reclaim_and_an_unpinned_one_goes() {
    let native = Native::new("reclaim-rollback-target");
    // The two earlier deliveries are as old as a timestamp can be, so the
    // installed one is the newest without choosing an age.
    deliver(&native, "0.1.0", b"stado 0.1.0\n", SystemTime::UNIX_EPOCH);
    deliver(&native, "0.2.0", b"stado 0.2.0\n", SystemTime::UNIX_EPOCH);
    deliver(&native, "0.3.0", b"stado 0.3.0\n", SystemTime::now());
    let bin = native.home.join(".stado/bin");
    fs::create_dir_all(&bin).unwrap();
    fs::write(bin.join("stado.release-version"), "0.3.0\n").unwrap();
    fs::write(bin.join("stado.previous"), b"stado 0.2.0\n").unwrap();

    // The stage takes a tree only on the second apply that finds it settled.
    reclaim(&native, "--apply");
    let applied = reclaim(&native, "--apply");

    let releases = native.home.join(".stado/releases/stado");
    assert!(
        releases.join("0.3.0").is_dir(),
        "the installed release was taken: {applied}"
    );
    assert!(
        releases.join("0.2.0").join(PLATFORM).join("stado").is_file(),
        "the rollback target's release was taken, leaving nothing to restore: {applied}"
    );
    assert!(
        !releases.join("0.1.0").exists(),
        "a release neither installed, newest nor the rollback target was kept: {applied}"
    );
}
