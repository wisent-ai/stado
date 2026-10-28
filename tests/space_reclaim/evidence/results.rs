//! The real local reclaim command on an isolated APFS volume, with both
//! Probierz evidence prefixes and protected delivery inputs.
#![cfg(target_os = "macos")]

#[path = "../native.rs"]
mod native;

use std::fs;
use std::os::unix::fs::symlink;

use native::Native;

/// The served queue namespace of `native`'s store, as a host whose object API
/// serves the store keeps it: Probierz's evidence prefixes live there, and so
/// does the registry every queue client on the host reads once that
/// directory exists (`StoreRoot::Client`).
fn served_namespace(native: &Native) -> std::path::PathBuf {
    let namespace = native.home.join(".stado/local-storage/ecosystem/probierz");
    fs::create_dir_all(&namespace).unwrap();
    fs::copy(&native.registry, namespace.join("registry.json")).unwrap();
    namespace
}

#[test]
fn dispatched_results_and_a_build_output_are_reclaimed_without_taking_release_records_or_inputs() {
    let native = Native::new("probierz-results");
    let store = served_namespace(&native);
    let result = store.join("results/journey/output.tar");
    let run = store.join("runs/probierz/journey/report.json");
    let release = store.join("runs/release-pipeline/stado/run/receipt.json");
    let pinned = store.join("results/artifacts/native-signing/input.tar");
    for path in [&result, &run, &release, &pinned] {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, b"retained source bytes").unwrap();
    }
    // A build output beside the evidence: the same triggered pass takes both,
    // so reclaiming evidence never stands in for the build-cache cleaner.
    let output = native.home.join("work/repo/target");
    native.cache(&output);
    let user_data = native.fill_with_user_data();
    let report = native.cleanup();
    assert_eq!(report["rule"]["triggered"], true, "{report}");
    assert!(!result.exists(), "dispatched result survived: {report}");
    assert!(!run.exists(), "run evidence survived: {report}");
    assert!(
        !output.exists(),
        "a build output survived the pass: {report}"
    );
    assert_eq!(
        fs::read(&release).unwrap(),
        b"retained source bytes",
        "{report}"
    );
    assert_eq!(
        fs::read(&pinned).unwrap(),
        b"retained source bytes",
        "{report}"
    );
    assert!(user_data.is_file(), "user data was taken: {report}");
}

#[test]
fn a_link_in_the_results_prefix_cannot_redirect_cleanup() {
    let native = Native::new("probierz-results-link");
    let store = served_namespace(&native);
    let outside = native.home.join("Documents/user-evidence.json");
    fs::create_dir_all(outside.parent().unwrap()).unwrap();
    fs::write(&outside, b"user-owned").unwrap();
    let results = store.join("results");
    fs::create_dir_all(results.parent().unwrap()).unwrap();
    symlink(outside.parent().unwrap(), &results).unwrap();
    let run = store.join("runs/probierz/journey/report.json");
    fs::create_dir_all(run.parent().unwrap()).unwrap();
    fs::write(&run, b"disposable").unwrap();
    native.fill_with_user_data();
    let report = native.cleanup();
    assert_eq!(report["rule"]["triggered"], true, "{report}");
    assert_eq!(fs::read(outside).unwrap(), b"user-owned", "{report}");
    assert!(
        !run.exists(),
        "the other evidence root was skipped: {report}"
    );
    assert!(results.is_symlink(), "the link was removed: {report}");
    assert!(
        report["cleaners"]["object_evidence"]["skipped"]["root_not_a_directory"]
            .as_u64()
            .is_some(),
        "the linked results root was not refused: {report}"
    );
}
