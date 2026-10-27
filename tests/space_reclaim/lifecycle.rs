//! The disk-full rule against a real attached volume, real local storage and
//! kernel locks, with no builds or clock-driven pacing performed by the test.
//!
//! macOS only: the isolated volume is an APFS disk image attached with
//! `hdiutil`, which needs no privileges there.
#![cfg(target_os = "macos")]

#[path = "guards/build_lock.rs"]
mod build_lock;
#[path = "guards/failed_release.rs"]
mod failed_release;
mod native;
#[path = "guards/product_hold.rs"]
mod product_hold;
mod retirement;
#[path = "guards/rollback_target.rs"]
mod rollback_target;
mod sourcekit;
mod space;
mod workdirs;
#[path = "guards/workload_hold.rs"]
mod workload_hold;

use std::fs;
use std::os::unix::fs::symlink;

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
    // The janitor's rule and the host gates read one declared threshold.
    assert_eq!(report["rule"]["full_percent"], native.disk()["full_percent"], "{report}");
    assert_eq!(report["rule"]["triggered"], false, "{report}");
    assert_eq!(report["outcome"], "healthy_noop", "{report}");
    assert!(
        cache.join("payload").is_file(),
        "a cache went below the threshold"
    );
    assert!(log.is_file(), "a log went below the threshold");
}

/// The Time Machine snapshots of the machine's boot data volume, as
/// `tmutil` lists them.
fn host_snapshots() -> Vec<String> {
    let output = std::process::Command::new("/usr/bin/tmutil")
        .args(["listlocalsnapshots", "/"])
        .output()
        .expect("tmutil runs");
    assert!(
        output.status.success(),
        "tmutil listlocalsnapshots / refused: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .filter(|line| line.starts_with("com.apple.TimeMachine."))
        .map(str::to_string)
        .collect()
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
    // `a=` grants no one anything, as chmod(1) reads the symbolic mode.
    let locked = std::process::Command::new("chmod")
        .arg("a=")
        .arg(&blocked)
        .status()
        .unwrap();
    assert!(locked.success(), "chmod a= {} answered {locked}", blocked.display());
    native.fill_with_user_data();
    let report = native.cleanup();
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
