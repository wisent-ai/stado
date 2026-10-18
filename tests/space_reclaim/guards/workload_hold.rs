//! A running workload's hold no longer keeps the janitor from reading the
//! disk: under pressure, with a workload's shared hold on the cleanup lock,
//! a real `stado space reclaim --local` pass on the isolated volume still
//! walks the home and finds the tagged cache, but keeps it, because the
//! removal takes the exclusive cleanup lock for that cache and the
//! workload holds it. Once the workload lets go, the next pass removes it.

use std::fs::{self, File};
use std::os::fd::AsRawFd;

use super::native::Native;

#[test]
fn a_workload_hold_keeps_the_cache_the_walk_found_and_a_free_pass_removes_it() {
    let native = Native::new("workload-hold");
    let caches = [native.home.join("checkout/target")];
    for cache in &caches {
        native.cache(cache);
    }
    native.fill_with_user_data();
    fs::create_dir_all(native.state_dir()).unwrap();
    let hold = File::options()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(native.state_dir().join("disk-cleanup.lock"))
        .unwrap();
    // SAFETY: a descriptor this test owns; the hold is released when `hold`
    // is dropped below. A workload takes the cleanup lock shared, as here.
    let held =
        unsafe { nix::libc::flock(hold.as_raw_fd(), nix::libc::LOCK_SH | nix::libc::LOCK_NB) };
    assert!(
        !held.is_negative(),
        "the test could not take a workload's hold"
    );

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
