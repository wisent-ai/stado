//! A tagged build directory a running build holds is kept, and one whose
//! build lock nobody holds is reclaimed by a real `stado space reclaim
//! --local` pass on the isolated volume. The test holds Cargo's build lock the way a
//! running `cargo build` does: an exclusive advisory lock on the profile
//! directory's `.cargo-lock`.

use std::fs::{self, File};
use std::os::fd::AsRawFd;

use super::native::Native;

#[test]
fn a_cache_a_running_build_holds_is_kept_and_a_free_one_reclaimed() {
    let native = Native::new("build-lock");
    // One running build per lock file a Cargo release holds in its profile
    // directory: a target directory's `.cargo-lock` and `.cargo-artifact-lock`,
    // and the `.cargo-build-lock` of a build-dir under Cargo's home, which
    // Cargo tags with CACHEDIR.TAG like a target directory.
    let held_by = [".cargo-lock", ".cargo-artifact-lock", ".cargo-build-lock"];
    let building: Vec<_> = held_by
        .iter()
        .enumerate()
        .map(|(index, _)| {
            native
                .home
                .join(format!("checkout-building-{index}/target"))
        })
        .collect();
    let idle = [native.home.join("checkout-idle/target")];
    for cache in building.iter().chain(idle.iter()) {
        native.cache(cache);
        fs::create_dir_all(cache.join("debug")).unwrap();
        for name in held_by {
            fs::write(cache.join("debug").join(name), b"").unwrap();
        }
    }
    let locks: Vec<File> = building
        .iter()
        .zip(held_by)
        .map(|(cache, name)| {
            let lock = File::options()
                .read(true)
                .write(true)
                .open(cache.join("debug").join(name))
                .unwrap();
            // SAFETY: a descriptor this test owns; the lock is released when
            // `locks` is dropped at the end of the test.
            let locked = unsafe {
                nix::libc::flock(lock.as_raw_fd(), nix::libc::LOCK_EX | nix::libc::LOCK_NB)
            };
            assert!(
                !locked.is_negative(),
                "the test could not hold the build lock {name}"
            );
            lock
        })
        .collect();
    native.fill_with_user_data();

    let report = native.cleanup();
    let builds = &report["cleaners"]["build_caches"];
    assert_eq!(report["rule"]["triggered"], true, "{report}");
    for cache in &building {
        assert!(
            cache.join("payload").is_file(),
            "a cache a running build holds was reclaimed: {report}"
        );
    }
    for cache in &idle {
        assert!(
            !cache.exists(),
            "a cache nobody builds in was kept: {report}"
        );
    }
    assert_eq!(builds["deleted_items"], idle.len(), "{report}");
    assert_eq!(
        builds["skipped"]["held_by_build"],
        building.len(),
        "{report}"
    );
    drop(locks);
}
