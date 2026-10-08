//! The disk-full rule against a real attached volume, real local storage and
//! kernel locks, with no builds or clock-driven pacing performed by the test.
//!
//! macOS only: the isolated volume is an APFS disk image attached with
//! `hdiutil`, which needs no privileges there.
#![cfg(target_os = "macos")]

mod native;
mod promise;
mod reclaim;
mod retirement;

use std::fs::{self, FileTimes, OpenOptions};
use std::os::unix::fs::{symlink, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::time::SystemTime;

use native::Native;
use serde_json::json;

#[test]
fn below_the_threshold_a_pass_deletes_nothing() {
    let native = Native::new("below-threshold");
    let cache = native.home.join("checkout/target");
    native.cache(&cache);
    let log = native.home.join(".omp/logs/request.json");
    fs::create_dir_all(log.parent().unwrap()).unwrap();
    fs::write(&log, b"{}").unwrap();
    let report = native.cleanup();
    assert_eq!(report["rule"]["full_percent"], 80);
    assert_eq!(report["rule"]["triggered"], false, "{report}");
    assert_eq!(report["outcome"], "healthy_noop", "{report}");
    assert!(
        cache.join("payload").is_file(),
        "a cache went below the threshold"
    );
    assert!(log.is_file(), "a log went below the threshold");
}

#[test]
fn at_the_threshold_everything_the_fleet_put_there_goes_and_user_data_stays() {
    let native = Native::new("at-threshold");
    let ssh_key = native.home.join(".ssh/id_ed25519");
    fs::create_dir_all(ssh_key.parent().unwrap()).unwrap();
    fs::write(&ssh_key, b"private key\n").unwrap();
    let notes = native.home.join("Documents/notes.txt");
    fs::create_dir_all(notes.parent().unwrap()).unwrap();
    fs::write(&notes, b"the user's own file\n").unwrap();
    let caches: Vec<_> = (0..3)
        .map(|index| native.home.join(format!("work/repo-{index}/target")))
        .collect();
    for cache in &caches {
        native.cache(cache);
    }
    let log = native.home.join(".omp/logs/request.json");
    fs::create_dir_all(log.parent().unwrap()).unwrap();
    fs::write(&log, b"{}").unwrap();
    let recording = native.home.join("weles/recordings/run-1/video.mp4");
    fs::create_dir_all(recording.parent().unwrap()).unwrap();
    fs::write(&recording, b"not uploaded anywhere").unwrap();
    let user_data = native.fill_with_user_data();

    let report = native.cleanup();
    assert_eq!(report["rule"]["triggered"], true, "{report}");
    assert_eq!(report["pressure_active"], true);
    assert_eq!(
        report["cleaners"]["build_caches"]["deleted_items"], 3,
        "{report}"
    );
    assert_eq!(
        report["cleaners"]["agent_logs"]["deleted_items"], 1,
        "{report}"
    );
    assert_eq!(
        report["cleaners"]["weles_recordings"]["deleted_items"], 1,
        "{report}"
    );
    for cache in &caches {
        assert!(!cache.exists(), "{} survived the rule", cache.display());
    }
    assert!(!log.exists(), "a harness log survived the rule");
    assert!(!recording.exists(), "a Weles recording survived the rule");
    assert_eq!(fs::read(&ssh_key).unwrap(), b"private key\n");
    assert_eq!(fs::read(&notes).unwrap(), b"the user's own file\n");
    assert!(user_data.is_file(), "the user's data was deleted");
    // The volume is still full of the user's data, which the rule never takes.
    assert_eq!(report["outcome"], "still_full", "{report}");
}

#[test]
fn at_the_threshold_cargo_input_caches_stay_and_its_build_output_goes() {
    // Cargo tags its package registry and its git dependency clones with the
    // same CACHEDIR.TAG as a target/ tree, but they are INPUT shared by every
    // build on the host, and deleting the git clones under a running cargo
    // failed every build with a git dependency
    // ("failed to create temporary file '~/.cargo/git/db/…'").
    let native = Native::new("cargo-inputs");
    let registry = native.home.join(".cargo/registry");
    let git = native.home.join(".cargo/git");
    let output = native.home.join("work/repo/target");
    for cache in [&registry, &git, &output] {
        native.cache(cache);
    }
    let user_data = native.fill_with_user_data();

    let report = native.cleanup();
    assert_eq!(report["rule"]["triggered"], true, "{report}");
    assert!(
        registry.join("payload").is_file(),
        "cargo's registry was deleted: {report}"
    );
    assert!(
        git.join("payload").is_file(),
        "cargo's git dependency cache was deleted: {report}"
    );
    assert!(!output.exists(), "a build output survived the rule");
    assert!(user_data.is_file(), "the user's data was deleted");
}

#[test]
fn at_the_threshold_run_evidence_and_the_compiler_cache_go_and_release_records_stay() {
    // The object store's probierz/runs prefix holds product run evidence and
    // Stado's own release records side by side. Taking the records deleted
    // every release run and build mid-delivery on the host serving the fleet
    // store; Kache's store was covered by no cleaner.
    let native = Native::new("runs-and-kache");
    let runs = native
        .home
        .join(".stado/local-storage/ecosystem/probierz/runs");
    let evidence = runs.join("probierz/journey-run/report.json");
    let kept: Vec<_> = [
        "release-pipeline/stado/run-a/run.json",
        "build/stado/build-a/request.json",
        "release-changes/batch-a.json",
        "artifacts/native-signing/apple-issuers.pem",
    ]
    .iter()
    .map(|path| runs.join(path))
    .collect();
    let kache = native.home.join("Library/Caches/kache/objects/entry.bin");
    for file in kept.iter().chain([&evidence, &kache]) {
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(file, b"recorded").unwrap();
    }
    let user_data = native.fill_with_user_data();

    let report = native.cleanup();
    assert_eq!(report["rule"]["triggered"], true, "{report}");
    assert!(
        !evidence.exists(),
        "product run evidence survived: {report}"
    );
    assert!(!kache.exists(), "the compiler cache survived: {report}");
    for file in &kept {
        assert_eq!(
            fs::read(file).unwrap(),
            b"recorded",
            "{} was taken: {report}",
            file.display()
        );
    }
    assert_eq!(
        report["cleaners"]["object_evidence"]["skipped"]["stado_record_or_pinned_input_kept"],
        json!(kept.len()),
        "{report}"
    );
    assert!(user_data.is_file(), "the user's data was deleted");
}

#[test]
fn an_aged_live_kernel_lock_is_not_replaced_and_release_allows_cleanup() {
    let native = Native::new("live-lock");
    let candidate = native.home.join("work/target");
    native.cache(&candidate);
    native.fill_with_user_data();
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
    native.observe(
        "held kernel lock",
        json!({
            "before": {"device": before.dev(), "inode": before.ino()},
            "after": {"device": after.dev(), "inode": after.ino()},
            "candidate_present": candidate.is_dir(),
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
    let candidate = native.home.join("work/target");
    native.cache(&candidate);
    native.fill_with_user_data();
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
fn a_symbolic_link_out_of_the_home_is_never_followed() {
    let native = Native::new("symlink-boundary");
    let outside = native.home.parent().unwrap().join("outside-home");
    native.cache(&outside);
    let original = fs::read(outside.join("payload")).unwrap();
    let link = native.home.join("work/outside-link");
    fs::create_dir_all(link.parent().unwrap()).unwrap();
    symlink(&outside, &link).unwrap();
    native.fill_with_user_data();
    native.cleanup();
    assert!(
        link.is_symlink(),
        "cleanup removed an ineligible symbolic link"
    );
    assert_eq!(
        fs::read(outside.join("payload")).unwrap(),
        original,
        "cleanup followed a link out of the home"
    );
    fs::remove_dir_all(&outside).unwrap();
}

#[test]
fn unreadable_cache_size_refuses_deletion_and_preserves_the_cause() {
    let native = Native::new("unreadable-size");
    assert!(
        !nix::unistd::geteuid().is_root(),
        "permission refusal requires an unprivileged native test process"
    );
    let candidate = native.home.join("work/target");
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
    native.fill_with_user_data();
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
