//! Native cleanup against real local storage and kernel locks, with no builds
//! or clock-driven pacing performed by the test itself.

mod migration;
mod native;

use std::fs::{self, FileTimes, OpenOptions};
use std::os::unix::fs::{symlink, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::time::SystemTime;

use native::Native;
use serde_json::{json, Value};

#[test]
fn an_aged_live_kernel_lock_is_not_replaced_and_release_allows_cleanup() {
    let native = Native::new("live-lock");
    let candidate = native.cache_root.join("eligible");
    native.cache(&candidate);
    let state_dir = native.state_dir();
    fs::create_dir_all(&state_dir).unwrap();
    let path = state_dir.join("disk-cleanup.lock");
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(&path)
        .unwrap();
    fs2::FileExt::try_lock_exclusive(&lock).unwrap();
    lock.set_times(FileTimes::new().set_modified(SystemTime::UNIX_EPOCH))
        .unwrap();
    let before = lock.metadata().unwrap();
    let report = native.cleanup();
    let after = path.metadata().unwrap();
    let retired: Vec<_> = fs::read_dir(&state_dir)
        .unwrap()
        .map(Result::unwrap)
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with("disk-cleanup.lock.retired.")
        })
        .map(|entry| entry.path())
        .collect();
    native.observe(
        "held kernel lock",
        json!({
            "before": {"device": before.dev(), "inode": before.ino()},
            "after": {"device": after.dev(), "inode": after.ino()},
            "candidate_present": candidate.is_dir(), "retired_paths": retired,
        }),
    );
    assert_eq!(
        (before.dev(), before.ino()),
        (after.dev(), after.ino()),
        "a live kernel lock was replaced because of its age"
    );
    assert_eq!(report["lock_busy"], true);
    assert!(
        candidate.is_dir(),
        "cleanup deleted while another process held its lock"
    );
    assert!(
        retired.is_empty(),
        "cleanup created a second lock generation"
    );
    drop(lock);
    native.cleanup();
    assert!(
        !candidate.exists(),
        "kernel release did not permit reclamation"
    );
}

#[test]
fn an_already_retired_locked_inode_remains_protected_until_kernel_release() {
    let native = Native::new("retired-lock");
    let candidate = native.cache_root.join("eligible");
    native.cache(&candidate);
    let state_dir = native.state_dir();
    fs::create_dir_all(&state_dir).unwrap();
    let path = state_dir.join("disk-cleanup.lock.retired.regression");
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&path)
        .unwrap();
    fs2::FileExt::try_lock_exclusive(&lock).unwrap();
    let held = native.cleanup();
    native.observe(
        "retired kernel lock",
        json!({"candidate_present": candidate.is_dir(), "retired_path_present": path.exists()}),
    );
    assert_eq!(held["outcome"], "lock_recovery_report_only");
    assert!(
        candidate.is_dir(),
        "cleanup bypassed a held predecessor inode"
    );
    drop(lock);
    native.cleanup();
    assert!(
        !candidate.exists(),
        "an unlocked predecessor still prevented reclamation"
    );
    assert!(
        !path.exists(),
        "the unlocked predecessor remained in persisted state"
    );
}

#[test]
fn item_bound_passes_make_progress_without_deleting_untagged_state() {
    let native = Native::new("item-bound");
    let first = native.cache_root.join("first");
    let second = native.cache_root.join("second");
    let keep = native.cache_root.join("keep");
    native.cache(&first);
    native.cache(&second);
    fs::write(&keep, b"not a regenerable cache\n").unwrap();
    let mut policy = native.policy.clone();
    policy["max_items_per_pass"] = json!(1);
    native.set_policy(&policy);
    let first_pass = native.cleanup();
    let remaining = usize::from(first.exists()) + usize::from(second.exists());
    native.observe("first bounded pass", json!({"remaining_caches": remaining}));
    assert_eq!(
        remaining, 1,
        "a one-item pass did not remove exactly one cache"
    );
    assert_eq!(first_pass["cleaners"]["build_caches"]["deleted_items"], 1);
    native.cleanup();
    assert!(
        !first.exists() && !second.exists(),
        "the next explicit pass did not reclaim the remaining cache"
    );
    assert_eq!(fs::read(&keep).unwrap(), b"not a regenerable cache\n");
}

#[test]
fn a_symbolic_link_does_not_authorize_cleanup_outside_the_declared_root() {
    let native = Native::new("symlink-boundary");
    let outside = native.home.join("outside-declared-root");
    native.cache(&outside);
    let original = fs::read(outside.join("payload")).unwrap();
    let link = native.cache_root.join("outside-link");
    symlink(&outside, &link).unwrap();
    native.cleanup();
    native.observe("symlink boundary", json!({"link_present": link.is_symlink(), "outside_payload_present": outside.join("payload").is_file()}));
    assert!(
        link.is_symlink(),
        "cleanup removed an ineligible symbolic link"
    );
    assert_eq!(
        fs::read(outside.join("payload")).unwrap(),
        original,
        "cleanup followed a link outside its root"
    );
}

#[test]
fn the_removed_pass_clock_is_refused_without_changing_canonical_state() {
    let native = Native::new("removed-pass-clock");
    let before = fs::read(&native.registry).unwrap();
    let response = native.run(&[
        "space",
        "watermark",
        "example-cleanup-host",
        "--disk-max-pass-seconds",
        "1",
        "--json",
    ]);
    let after = fs::read(&native.registry).unwrap();
    native.observe(
        "obsolete clock refusal",
        json!({"exit_status": response.status.code(), "registry_unchanged": before == after}),
    );
    assert_eq!(
        response.status.code(),
        Some(2),
        "the removed setting was accepted: {}",
        String::from_utf8_lossy(&response.stdout)
    );
    assert!(
        String::from_utf8_lossy(&response.stderr).contains("--disk-max-pass-seconds"),
        "the invocation refusal did not name the rejected setting"
    );
    assert_eq!(before, after, "a refused setting changed persisted policy");
    let document: Value = serde_json::from_slice(&after).unwrap();
    assert_eq!(document["targets"][0]["disk_cleanup"], native.policy);
}

#[test]
fn unreadable_cache_size_refuses_deletion_and_preserves_the_cause() {
    let native = Native::new("unreadable-size");
    assert!(
        !nix::unistd::geteuid().is_root(),
        "permission refusal requires an unprivileged native test process"
    );
    let candidate = native.cache_root.join("eligible");
    native.cache(&candidate);
    let blocked = candidate.join("blocked");
    fs::create_dir(&blocked).unwrap();
    fs::write(
        blocked.join("payload"),
        b"must remain unread until permission is restored",
    )
    .unwrap();
    let original = fs::metadata(&blocked).unwrap().permissions();
    struct Restore(std::path::PathBuf, fs::Permissions);
    impl Drop for Restore {
        fn drop(&mut self) {
            if let Err(error) = fs::set_permissions(&self.0, self.1.clone()) {
                eprintln!("restore native cache-test permissions: {error}");
            }
        }
    }
    let restore = Restore(blocked.clone(), original);
    fs::set_permissions(&blocked, fs::Permissions::from_mode(0o000)).unwrap();
    let old = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1);
    fs::File::open(&candidate)
        .unwrap()
        .set_times(FileTimes::new().set_modified(old))
        .unwrap();
    let report = native.cleanup();
    assert_eq!(report["cleaners"]["build_caches"]["deleted_items"], 0);
    assert!(
        report["errors"].as_array().unwrap().iter().any(|error| {
            error.as_str().is_some_and(|message| {
                message.contains("PermissionError") && message.contains("blocked")
            })
        }),
        "{report}"
    );
    drop(restore);
    assert_eq!(
        fs::read(blocked.join("payload")).unwrap(),
        b"must remain unread until permission is restored"
    );
    assert!(candidate.join("CACHEDIR.TAG").is_file());
}
