//! The run evidence cleaner keeps deletion within its area: at the disk-full
//! threshold, symbolic links under or in any component of the object store's
//! `probierz/runs` prefix are kept and never walked. The user files those
//! links reach stay as they were, while evidence inside the area goes.
//! A home directory named like a pinned-input directory does not make
//! unrelated evidence beneath the root immutable.

use std::fs;
use std::os::unix::fs::symlink;

use serde_json::{json, Value};

use crate::native::Native;

const RUNS: &str = ".stado/local-storage/ecosystem/probierz/runs";

#[test]
fn a_link_inside_the_evidence_area_is_kept_and_its_target_is_never_touched() {
    let native = Native::new("evidence-link");
    let runs = native.home.join(RUNS);
    let evidence = runs.join("probierz/journey-run/report.json");
    fs::create_dir_all(evidence.parent().unwrap()).unwrap();
    fs::write(&evidence, b"recorded").unwrap();
    let outside = native.home.join("Documents/operator-data");
    let sentinel = outside.join("notes.txt");
    fs::create_dir_all(&outside).unwrap();
    fs::write(&sentinel, b"the operator's own").unwrap();
    let links = [runs.join("probierz/linked-run")];
    for link in &links {
        symlink(&outside, link).unwrap();
    }
    let user_data = native.fill_with_user_data();

    let report = native.cleanup();
    assert_eq!(report["rule"]["triggered"], true, "{report}");
    assert!(
        !evidence.exists(),
        "run evidence inside the area survived: {report}"
    );
    for link in &links {
        assert!(
            link.is_symlink(),
            "the link under the area was removed: {report}"
        );
    }
    assert_eq!(
        fs::read(&sentinel).unwrap(),
        b"the operator's own",
        "the cleaner followed a link out of its area: {report}"
    );
    assert_eq!(
        report["cleaners"]["object_evidence"]["skipped"]["not_a_regular_file"],
        json!(links.len()),
        "{report}"
    );
    assert!(user_data.is_file(), "the user's data was deleted");
}

#[test]
fn a_link_in_the_evidence_area_s_own_place_is_not_followed() {
    let native = Native::new("evidence-root-link");
    let roots = [native.home.join(RUNS)];
    let outside = native.home.join("Documents/elsewhere");
    let sentinel = outside.join("probierz/journey-run/report.json");
    fs::create_dir_all(sentinel.parent().unwrap()).unwrap();
    fs::write(&sentinel, b"not evidence").unwrap();
    for root in &roots {
        fs::create_dir_all(root.parent().unwrap()).unwrap();
        symlink(&outside, root).unwrap();
    }
    native.fill_with_user_data();

    let report = native.cleanup();
    assert_eq!(report["rule"]["triggered"], true, "{report}");
    assert!(
        roots.iter().all(|root| root.is_symlink()),
        "the link standing in the area's place was removed: {report}"
    );
    assert_eq!(
        fs::read(&sentinel).unwrap(),
        b"not evidence",
        "the cleaner walked a link standing in its area's place: {report}"
    );
    assert_eq!(
        report["cleaners"]["object_evidence"]["skipped"]["root_not_a_directory"],
        json!(roots.len()),
        "{report}"
    );
}

#[test]
fn a_link_in_an_ancestor_of_the_evidence_area_is_not_followed() {
    let native = Native::new("evidence-ancestor-link");
    let links = [native.home.join(".stado/local-storage/ecosystem/probierz")];
    let outside = native.home.join("Documents/elsewhere");
    let sentinel = outside.join("runs/probierz/journey-run/report.json");
    fs::create_dir_all(sentinel.parent().unwrap()).unwrap();
    fs::write(&sentinel, b"not evidence").unwrap();
    for link in &links {
        fs::create_dir_all(link.parent().unwrap()).unwrap();
        symlink(&outside, link).unwrap();
    }
    native.fill_with_user_data();

    let report = native.cleanup();
    assert_eq!(report["rule"]["triggered"], true, "{report}");
    assert!(
        links.iter().all(|link| link.is_symlink()),
        "an ancestor link was removed: {report}"
    );
    assert_eq!(
        fs::read(&sentinel).unwrap(),
        b"not evidence",
        "the cleaner walked an ancestor link into user data: {report}"
    );
    assert_eq!(
        report["cleaners"]["object_evidence"]["skipped"]["root_not_a_directory"],
        json!(links.len()),
        "{report}"
    );
}

#[test]
fn a_home_named_like_pinned_inputs_does_not_protect_unrelated_evidence() {
    let native = Native::new("evidence-home-name");
    let home = native.home.join("native-signing");
    let evidence = home.join(RUNS).join("probierz/journey-run/report.json");
    fs::create_dir_all(evidence.parent().unwrap()).unwrap();
    fs::write(&evidence, b"disposable evidence").unwrap();
    let user_data = native.fill_with_user_data();
    let config = home.join(".stado/config.json");
    let run_nested = |args: &[&str]| {
        let output = native
            .command(args)
            .env("HOME", &home)
            .env("STADO_CONFIG", &config)
            .output()
            .unwrap();
        native.record(env!("CARGO_BIN_EXE_stado"), args, &output);
        assert!(
            output.status.success(),
            "stado {args:?} refused: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        output
    };
    run_nested(&["config", "init"]);
    let ceiling = f64::MAX.to_string();
    run_nested(&["config", "set", "host.scan_load_ceiling", &ceiling]);
    let args = [
        "space",
        "reclaim",
        "--local",
        "--apply",
        "--reason",
        "isolated cleanup test",
    ];
    let output = run_nested(&args);
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    native.observe("cleanup under nested home", report.clone());
    let state = home.join(".cache/wisent-compute/space-reclaim-state.json");
    let persisted: Value = serde_json::from_slice(&fs::read(&state).unwrap()).unwrap();
    assert_eq!(
        persisted["report"]["cleaners"]["object_evidence"]["deleted_items"],
        report["cleaners"]["object_evidence"]["deleted_items"],
        "the cleanup result was not saved: {persisted}"
    );
    native.observe(
        "nested persisted state",
        json!({"path": state, "state": persisted}),
    );
    assert_eq!(report["rule"]["triggered"], true, "{report}");
    assert!(!evidence.exists(), "disposable evidence survived: {report}");
    assert!(user_data.is_file(), "the user's data was deleted: {report}");
}

#[test]
fn protected_release_records_are_not_opened_or_counted_as_run_evidence() {
    let native = Native::new("evidence-opaque-records");
    let runs = native.home.join(RUNS);
    let records = runs.join("release-pipeline");
    let kept = [
        records.join("stado/run-a/run.json"),
        records.join("stado/run-a/details/receipt.json"),
    ];
    for file in &kept {
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(file, b"release record").unwrap();
    }
    let user_data = native.fill_with_user_data();

    let report = native.cleanup();
    assert_eq!(report["rule"]["triggered"], true, "{report}");
    for file in &kept {
        assert_eq!(fs::read(file).unwrap(), b"release record", "{report}");
    }
    assert_eq!(
        report["cleaners"]["object_evidence"]["scanned_items"],
        json!(fs::read_dir(&runs).unwrap().count()),
        "the cleaner opened a protected subtree: {report}"
    );
    assert_eq!(
        report["cleaners"]["object_evidence"]["skipped"]["stado_record_or_pinned_input_kept"],
        json!(fs::read_dir(&runs).unwrap().count()),
        "the protected subtree was not identified: {report}"
    );
    assert!(user_data.is_file(), "the user's data was deleted");
}
