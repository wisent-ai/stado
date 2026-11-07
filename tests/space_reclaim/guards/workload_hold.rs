//! A running workload's hold no longer keeps the janitor from reading the
//! disk: under pressure, with a workload's shared hold on the cleanup lock,
//! a real `stado space reclaim --local` pass on the isolated volume still
//! walks the home and finds the tagged cache, but keeps it, because the
//! removal takes the exclusive cleanup lock for that cache and the
//! workload holds it. Once the workload lets go, the next pass removes it.
//! A hold a workload recorded no longer stops every cleaner: beside it the
//! pass removes an older binary backup, records it, and names the cleaners
//! it left for its turn; while another pass runs those cleaners, it runs none.

use std::fs::{self, File};
use std::os::fd::AsRawFd;
use std::time::{Duration, SystemTime};

use serde_json::json;

use super::native::Native;

/// A shared hold on the cleanup lock, as a workload takes it; with `record`,
/// the hold record a workload writes beside the lock, naming this process.
fn shared_hold(native: &Native, record: bool) -> File {
    fs::create_dir_all(native.state_dir()).unwrap();
    let hold = File::options()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(native.state_dir().join("disk-cleanup.lock"))
        .unwrap();
    // SAFETY: a descriptor this test owns, released when the file drops.
    let held =
        unsafe { nix::libc::flock(hold.as_raw_fd(), nix::libc::LOCK_SH | nix::libc::LOCK_NB) };
    assert!(
        !held.is_negative(),
        "the test could not take a workload's hold"
    );
    if record {
        let holds = native.state_dir().join("disk-cleanup-holds");
        fs::create_dir_all(&holds).unwrap();
        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap();
        let body = json!({"pid": std::process::id(), "holder": "workload-hold test", "acquired_at": now.as_secs_f64()});
        fs::write(
            holds.join(format!("{}-test.json", std::process::id())),
            body.to_string(),
        )
        .unwrap();
    }
    hold
}

/// The unheld-cleaners lock taken exclusively, as a pass running those
/// cleaners beside workload holds takes it.
fn unheld_lock(native: &Native) -> File {
    let unheld = File::options()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(native.state_dir().join("disk-cleanup-unheld.lock"))
        .unwrap();
    // SAFETY: a descriptor this test owns, released when the file drops.
    let taken = unsafe { nix::libc::flock(unheld.as_raw_fd(), nix::libc::LOCK_EX) };
    assert_eq!(taken, 0, "the test could not take the unheld-cleaners lock");
    unheld
}

#[test]
fn a_pass_beside_workload_holds_is_named_and_not_counted_as_a_job() {
    let native = Native::new("workload-hold-janitor-pass");
    native.fill_with_user_data();
    // What a pass beside workload holds holds, with no workload left: the
    // run lock shared, unrecorded, and the unheld-cleaners lock.
    let shared = shared_hold(&native, false);
    let unheld = unheld_lock(&native);
    let report = native.cleanup();
    assert_eq!(report["outcome"], "lock_busy", "{report}");
    assert_eq!(report["active_job_count"], 0, "{report}");
    let errors = report["errors"].as_array().cloned().unwrap_or_default();
    assert!(
        errors
            .iter()
            .filter_map(|e| e.as_str())
            .any(|e| e.starts_with("lock_busy:")
                && e.contains("held in shared mode by a janitor pass running bin_backups")),
        "a pass beside workload holds is not named: {report}"
    );
    let waited = report["waited_cleaners"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(
        [
            "bin_backups",
            "delivered_releases",
            "package_caches",
            "queue_workdirs"
        ]
        .iter()
        .all(|name| waited.iter().any(|w| w == name)),
        "a busy pass does not name the cleaners it did not run: {report}"
    );
    drop(unheld);
    drop(shared);
}

#[test]
fn an_unheld_lock_that_cannot_be_probed_is_said_beside_an_unattributed_holder() {
    let native = Native::new("workload-hold-unheld-link");
    native.fill_with_user_data();
    let shared = shared_hold(&native, false);
    // The unheld-cleaners lock replaced by a link, which no lock open follows.
    let target = native.home.join("elsewhere.lock");
    fs::write(&target, b"").unwrap();
    std::os::unix::fs::symlink(&target, native.state_dir().join("disk-cleanup-unheld.lock"))
        .unwrap();
    let report = native.cleanup();
    assert_eq!(report["outcome"], "lock_busy_unattributed", "{report}");
    let errors = report["errors"].as_array().cloned().unwrap_or_default();
    assert!(
        errors
            .iter()
            .filter_map(|e| e.as_str())
            .any(|e| e.starts_with("unheld_lock:") && e.contains("disk-cleanup-unheld.lock")),
        "the probe's failure is not said: {report}"
    );
    drop(shared);
}

#[test]
fn a_recorded_workload_hold_still_lets_the_unheld_cleaners_run() {
    let native = Native::new("workload-hold-unheld");
    let bin = native.home.join(".stado/bin");
    fs::create_dir_all(&bin).unwrap();
    let backup = |name: &str, age: u64| {
        let path = bin.join(name);
        fs::write(&path, vec![b'b'; 64 * 1024]).unwrap();
        let modified = SystemTime::now() - Duration::from_secs(age);
        File::open(&path).unwrap().set_modified(modified).unwrap();
        path
    };
    backup("widget", 0);
    let newest = backup("widget.release-backup-20261103", 100);
    let older = backup("widget.release-backup-20261001", 200);
    native.fill_with_user_data();
    let hold = shared_hold(&native, true);

    // Another pass running the unheld cleaners holds their lock: this one
    // runs none of them and says why.
    let unheld = unheld_lock(&native);
    let refused = native.cleanup();
    assert_eq!(refused["outcome"], "lock_busy_workloads", "{refused}");
    assert!(
        older.is_file(),
        "a pass ran the unheld cleaners beside another: {refused}"
    );
    let errors = refused["errors"].as_array().cloned().unwrap_or_default();
    assert!(
        errors
            .iter()
            .filter_map(|e| e.as_str())
            .any(|e| e.starts_with("unheld_cleaners:BlockingIOError")),
        "the pass does not say why it ran no cleaner: {refused}"
    );
    assert!(
        refused["waited_cleaners"]
            .as_array()
            .is_some_and(|w| w.iter().any(|n| n == "bin_backups")),
        "a cleaner that did not run is not named as waiting: {refused}"
    );
    drop(unheld);

    let report = native.cleanup();
    assert_eq!(report["rule"]["triggered"], true, "{report}");
    assert_eq!(report["outcome"], "lock_busy_workloads", "{report}");
    assert_eq!(
        report["cleaners"]["bin_backups"]["deleted_items"], 1,
        "{report}"
    );
    assert!(
        !older.exists(),
        "an older backup stayed under a workload hold: {report}"
    );
    assert!(newest.is_file(), "the newest backup was taken: {report}");
    let removals = fs::read_to_string(native.state_dir().join("reclaimed.jsonl")).unwrap();
    assert!(
        removals.lines().any(|line| {
            serde_json::from_str::<serde_json::Value>(line)
                .is_ok_and(|row| row["cleaner"] == "bin_backups" && row["items"] == 1)
        }),
        "the removal beside the hold is not in the removal record: {removals}"
    );
    let waited = report["waited_cleaners"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(
        waited.iter().any(|name| name == "queue_workdirs")
            && !waited
                .iter()
                .any(|name| name == "bin_backups" || name == "local_snapshots"),
        "the cleaners left for the turn are named, the ones that ran are not: {report}"
    );
    // Time Machine's snapshots go beside the hold at the rule: only the
    // cleaner names the volume it read; the isolated one has no snapshot.
    assert!(
        report["local_snapshot_volume"].is_string(),
        "local_snapshots did not run beside the hold: {report}"
    );
    assert_eq!(
        report["cleaners"]["local_snapshots"]["deleted_items"], 0,
        "{report}"
    );
    drop(hold);
}

#[test]
fn a_workload_hold_keeps_the_cache_the_walk_found_and_a_free_pass_removes_it() {
    let native = Native::new("workload-hold");
    let caches = [native.home.join("checkout/target")];
    for cache in &caches {
        native.cache(cache);
    }
    native.fill_with_user_data();
    let hold = shared_hold(&native, false);

    let report = native.cleanup();
    let builds = &report["cleaners"]["build_caches"];
    assert_eq!(report["rule"]["triggered"], true, "{report}");
    assert_eq!(
        builds["eligible_items"],
        caches.len(),
        "the walk did not run beside the workload: {report}"
    );
    assert_eq!(
        builds["skipped"]["cleanup_lock_busy"],
        caches.len(),
        "{report}"
    );
    for cache in &caches {
        assert!(
            cache.join("payload").is_file(),
            "a cache was removed under a workload's hold: {report}"
        );
    }

    drop(hold);
    let report = native.cleanup();
    let builds = &report["cleaners"]["build_caches"];
    assert_eq!(builds["deleted_items"], caches.len(), "{report}");
    for cache in &caches {
        assert!(!cache.exists(), "a cache nobody holds was kept: {report}");
    }
}
