//! Declaring and withdrawing a cleaner, and what a refused write leaves
//! behind: the registry exactly as it was.

use std::fs;

use serde_json::Value;

use crate::fixture::{Host, TARGET};

use super::registry;

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

