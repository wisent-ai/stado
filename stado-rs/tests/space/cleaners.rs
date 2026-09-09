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
