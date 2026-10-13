//! A product build holds the janitor's cleanup lock, as every agent job does:
//! the real `stado product install` in the isolated home is refused, before
//! anything is exported or built, while a cleanup pass holds that lock
//! exclusively, and the refusal names the janitor. Once the pass lets go the
//! same command gets past the hold (and stops later, on what the isolated
//! home does not hold, which is not the janitor).

use std::fs::{self, File};
use std::os::fd::AsRawFd;

use super::native::Native;

const INSTALL: &[&str] = &["product", "install", "tama", "--surface", "cli"];

#[test]
fn a_product_install_waits_for_no_cleanup_pass_and_names_the_one_holding_the_lock() {
    let native = Native::new("product-hold");
    fs::create_dir_all(native.state_dir()).unwrap();
    let lock = File::options()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(native.state_dir().join("disk-cleanup.lock"))
        .unwrap();
    // SAFETY: a descriptor this test owns; the lock is released when `lock`
    // is dropped below.
    let locked =
        unsafe { nix::libc::flock(lock.as_raw_fd(), nix::libc::LOCK_EX | nix::libc::LOCK_NB) };
    assert!(
        !locked.is_negative(),
        "the test could not hold the cleanup lock"
    );

    let refused = native.run(INSTALL);
    let said = String::from_utf8_lossy(&refused.stderr).into_owned();
    native.observe(
        "install while a pass holds the lock",
        serde_json::json!({ "status": refused.status.code(), "stderr": said }),
    );
    assert!(!refused.status.success(), "{said}");
    assert!(said.contains("disk janitor"), "{said}");
    assert!(said.contains("nothing was exported or built"), "{said}");

    drop(lock);
    let admitted = native.run(INSTALL);
    let said = String::from_utf8_lossy(&admitted.stderr).into_owned();
    native.observe(
        "install once the pass let go",
        serde_json::json!({ "status": admitted.status.code(), "stderr": said }),
    );
    assert!(!said.contains("disk janitor"), "{said}");
}
