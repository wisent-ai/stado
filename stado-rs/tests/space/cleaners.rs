//! Cleaner writes and bounded cleanup through the real product binary.

use crate::fixture::{Host, TARGET};
use serde_json::{json, Value};
use std::fs;

fn registry(host: &Host) -> Value {
    serde_json::from_slice(&fs::read(host.storage.join("registry.json")).unwrap()).unwrap()
}

#[test]
fn declare_preserves_omitted_fields_and_refusals_leave_the_registry_unchanged() {
    let host = Host::new();
    // Deliberately stale desired state must not impersonate the installed binary.
    host.declare_running(&host.policy(), "0.1.0");
    let listing = host.json(&["space", "cleaners", "list", TARGET, "--json"]);
    assert_eq!(listing["installed_stado"], env!("CARGO_PKG_VERSION"));
    let root = host.home.join("release-scope");
    fs::create_dir_all(&root).unwrap();
    let root_text = root.to_str().unwrap();
    host.json(&[
        "space",
        "cleaners",
        "declare",
        TARGET,
        "--cleaner",
        "release_store",
        "--root",
        root_text,
        "--keep-newest",
        "2",
        "--json",
    ]);
    host.json(&[
        "space",
        "cleaners",
        "declare",
        TARGET,
        "--cleaner",
        "release_store",
        "--min-age-seconds",
        "60",
        "--json",
    ]);
    let current = registry(&host);
    let cleaner = &current["targets"][0]["disk_cleanup"]["cleaners"]["release_store"];
    assert_eq!(cleaner["root"], root_text);
    assert_eq!(cleaner["keep_newest"], 2);
    assert_eq!(cleaner["min_age_seconds"], 60);
    for fields in [
        vec!["--cleaner", "not_implemented"],
        vec!["--cleaner", "release_store", "--keep-newest", "0"],
        vec![
            "--cleaner",
            "release_store",
            "--allow-missing-upload-proof",
            "true",
        ],
    ] {
        let before = fs::read(host.storage.join("registry.json")).unwrap();
        let mut args = vec!["space", "cleaners", "declare", TARGET];
        args.extend(fields);
        assert!(!host.run(&args).status.success());
        assert_eq!(
            fs::read(host.storage.join("registry.json")).unwrap(),
            before
        );
    }
}

#[test]
fn remove_withdraws_only_the_named_cleaner_and_refuses_a_second_removal() {
    let host = Host::new();
    host.json(&[
        "space",
        "cleaners",
        "declare",
        TARGET,
        "--cleaner",
        "backup_twins",
        "--json",
    ]);
    host.json(&[
        "space",
        "cleaners",
        "remove",
        TARGET,
        "--cleaner",
        "backup_twins",
        "--json",
    ]);
    let value = registry(&host);
    assert!(value["targets"][0]["disk_cleanup"]["cleaners"]
        .get("backup_twins")
        .is_none());
    assert!(value["targets"][0]["disk_cleanup"]["cleaners"]
        .get("build_caches")
        .is_some());
    let before = fs::read(host.storage.join("registry.json")).unwrap();
    assert!(!host
        .run(&[
            "space",
            "cleaners",
            "remove",
            TARGET,
            "--cleaner",
            "backup_twins"
        ])
        .status
        .success());
    assert_eq!(
        fs::read(host.storage.join("registry.json")).unwrap(),
        before
    );
}

#[test]
fn an_unobserved_installed_version_cannot_authorize_a_cleaner_write() {
    let host = Host::new();
    host.declare_running(&host.policy(), env!("CARGO_PKG_VERSION"));
    fs::remove_file(host.home.join(".stado/bin/stado")).unwrap();
    let before = fs::read(host.storage.join("registry.json")).unwrap();
    let output = host.run(&[
        "space",
        "cleaners",
        "declare",
        TARGET,
        "--cleaner",
        "release_store",
    ]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("cannot verify cleaner support"));
    assert_eq!(
        fs::read(host.storage.join("registry.json")).unwrap(),
        before
    );
}

#[test]
fn bounded_replica_passes_reach_duplicates_beyond_a_retained_prefix() {
    let host = Host::new();
    let mut policy: Value = serde_json::from_str(&host.policy()).unwrap();
    policy["max_scan_items"] = json!(3);
    policy["max_items_per_pass"] = json!(1);
    policy["cleaners"] = json!({});
    policy["cleaners"]["backup_twins"] = json!({});
    policy["cleaners"]["backup_twins"]["min_age_seconds"] = json!(0);
    host.declare(&policy.to_string());
    let backup = host.home.join(".stado/local-backup");
    let primary = host
        .home
        .join(".stado/local-storage/ecosystem/replica-resume");
    fs::create_dir_all(&backup).unwrap();
    fs::create_dir_all(&primary).unwrap();
    for index in 0..6 {
        fs::write(
            backup.join(format!("a-retained-{index}")),
            b"only replica has this",
        )
        .unwrap();
    }
    let twin = backup.join("ecosystem/replica-resume/z-duplicate");
    fs::create_dir_all(twin.parent().unwrap()).unwrap();
    fs::write(&twin, b"same bytes on both sides").unwrap();
    fs::write(primary.join("z-duplicate"), b"same bytes on both sides").unwrap();
    let pass = || {
        let output = host.run(&["disk-cleanup", "--to-target"]);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        eprintln!("{}", String::from_utf8_lossy(&output.stdout));
    };
    pass();
    assert!(
        twin.exists(),
        "the first bounded pass should only see the retained prefix"
    );
    for _ in 0..4 {
        pass();
    }
    assert!(
        !twin.exists(),
        "resumed passes must not restart at retained files forever"
    );
    assert_eq!(
        fs::read(primary.join("z-duplicate")).unwrap(),
        b"same bytes on both sides"
    );
    for index in 0..6 {
        assert_eq!(
            fs::read(backup.join(format!("a-retained-{index}"))).unwrap(),
            b"only replica has this"
        );
    }
}

#[cfg(target_os = "macos")]
#[test]
fn the_named_reaper_stops_a_program_running_from_a_scratch_directory() {
    use std::process::{Child, Command};
    struct Program(Child);
    impl Drop for Program {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let host = Host::new();
    let directory = host.home.join(".stado/work/process-probe");
    fs::create_dir_all(&directory).unwrap();
    let program = directory.join("stado");
    fs::hard_link(env!("CARGO_BIN_EXE_stado"), &program).unwrap();
    let mut child = Program(
        Command::new(&program)
            .args(["agent", "--target", TARGET])
            .env_clear()
            .env("HOME", &host.home)
            .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
            .env("WC_STORAGE_BACKEND", "local")
            .env("WC_LOCAL_STORAGE_PATH", &host.storage)
            .env("WC_STADO_STORAGE_NAMESPACE", "space-fixture")
            .env("WC_PROVIDERS", "local")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap(),
    );
    let pattern = program.to_str().unwrap();
    let pid = child.0.id().to_string();
    let preview = host.json(&[
        "service",
        "reap",
        "--host",
        TARGET,
        "--command",
        pattern,
        "--json",
    ]);
    assert!(
        preview["reaped"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["pid"].as_str() == Some(pid.as_str()) && row["outcome"] == "would_end"),
        "{preview}"
    );
    assert!(
        child.0.try_wait().unwrap().is_none(),
        "preview terminated the process"
    );
    let applied = host.json(&[
        "service",
        "reap",
        "--host",
        TARGET,
        "--command",
        pattern,
        "--apply",
        "--json",
    ]);
    assert!(
        applied["reaped"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["pid"].as_str() == Some(pid.as_str()) && row["outcome"] == "ended"),
        "{applied}"
    );
    assert!(
        !child.0.wait().unwrap().success(),
        "the running program was not terminated"
    );
    assert!(
        program.is_file(),
        "process retirement should not remove files itself"
    );
}

/// The pass counts the local snapshots holding this volume's deleted blocks,
/// and a planning pass removes none of them.
///
/// The cleaner exists because a deletion on macOS frees nothing while a local
/// Time Machine snapshot still references the blocks: on a fleet Mac on
/// 2026-09-21 a pass removed 54 tagged build trees and `df` read 12.4 GiB free
/// before and after, and only thinning eleven snapshots moved it to 35.8 GiB.
/// So the janitor's own pass has to reach them — which is what this case
/// proves, through the planning pass the product runs with an enforcing policy
/// pinned down to `report`: every snapshot this machine holds is counted and
/// named eligible against a target far above its free space, and not one is
/// deleted. An operator's backup history is not test material, so the case
/// that would delete is the declared target itself, asserted here as the
/// number the cleaner measured against.
#[cfg(target_os = "macos")]
#[test]
fn a_planning_pass_counts_the_local_snapshots_and_deletes_none() {
    let host = Host::new();
    let mut policy: Value = serde_json::from_str(&host.policy()).unwrap();
    policy["cleaners"] = json!({"local_snapshots": {"min_age_seconds": 0}});
    host.declare(&policy.to_string());

    let report = host.json(&["disk-cleanup", "--dry-run"]);
    let snapshots = &report["cleaners"]["local_snapshots"];
    assert!(
        !snapshots.is_null(),
        "the pass never reached the snapshot cleaner: {report}"
    );
    assert_eq!(
        snapshots["deleted_items"].as_i64(),
        Some(0),
        "a planning pass deleted a snapshot: {report}"
    );
    let scanned = snapshots["scanned_items"].as_i64().unwrap_or_default();
    let listed = local_snapshot_count();
    assert_eq!(
        scanned, listed,
        "the pass counted {scanned} snapshot(s) where tmutil lists {listed}: {report}"
    );
    if scanned > 0 {
        assert_eq!(
            snapshots["eligible_items"].as_i64(),
            Some(scanned),
            "a volume below its declared target held snapshots the pass called ineligible: {report}"
        );
    }
}

/// What `tmutil` itself says this volume holds, Time Machine snapshots only.
#[cfg(target_os = "macos")]
fn local_snapshot_count() -> i64 {
    let listed = std::process::Command::new("/usr/bin/tmutil")
        .args(["listlocalsnapshots", "/"])
        .output()
        .expect("the real tmutil runs");
    String::from_utf8_lossy(&listed.stdout)
        .lines()
        .filter(|line| line.trim().starts_with("com.apple.TimeMachine."))
        .count() as i64
}
