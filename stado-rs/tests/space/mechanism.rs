//! Real directory sizes and policy overrides, read through the product binary.

use crate::fixture::{Host, TARGET};
use crate::system::du_bytes;
use serde_json::Value;
use std::fs;

fn coverage(host: &Host) -> Value {
    host.json(&["space", "report", TARGET, "--json"])["coverage"].clone()
}

#[test]
fn a_release_cleaner_never_claims_the_other_bytes_in_its_parent_store() {
    let host = Host::new();
    let store = host.home.join(".stado/local-storage");
    let releases = store.join("ecosystem/releases");
    fs::create_dir_all(&releases).unwrap();
    host.seed_tree(&releases, "retained-release", 8, false);
    let unrelated = host.seed_tree(&store, "operator-data", 16, false);
    let expected = du_bytes(&releases);
    let before = coverage(&host);
    let unarmed = before["unarmed"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["cleaner"] == "release_store")
        .expect("the measured release root must be named");
    assert_eq!(unarmed["bytes"].as_i64(), Some(expected));
    host.json(&[
        "space",
        "cleaners",
        "declare",
        TARGET,
        "--cleaner",
        "release_store",
        "--json",
    ]);
    let after = coverage(&host);
    let scope = after["cleaner_scopes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["cleaner"] == "release_store")
        .unwrap();
    assert_eq!(scope["root"], releases.to_str().unwrap());
    assert_eq!(scope["bytes"].as_i64(), Some(expected));
    assert_eq!(after["cleaner_bytes"].as_i64(), Some(expected));
    for row in after["uncovered"].as_array().unwrap() {
        if row["mechanism"] == "release_store" {
            assert!(std::path::Path::new(row["path"].as_str().unwrap()).starts_with(&releases));
        }
    }
    assert!(
        after["reclaimable_bytes"].is_null(),
        "a directory size cannot prove deletion eligibility"
    );
    assert!(unrelated.join("payload.bin").is_file());
}

#[test]
fn a_declared_root_override_replaces_the_default_scope_without_changing_files() {
    let host = Host::new();
    let default_root = host.home.join(".stado/local-backup");
    fs::create_dir_all(&default_root).unwrap();
    let retained = host.seed_tree(&default_root, "untouched", 8, false);
    let selected = host.seed_tree(&host.home, "selected-replicas", 4, false);
    host.json(&[
        "space",
        "cleaners",
        "declare",
        TARGET,
        "--cleaner",
        "backup_twins",
        "--root",
        selected.to_str().unwrap(),
        "--json",
    ]);
    let report = coverage(&host);
    let scope = report["cleaner_scopes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["cleaner"] == "backup_twins")
        .unwrap();
    assert_eq!(scope["root"], selected.to_str().unwrap());
    assert_eq!(report["cleaner_bytes"].as_i64(), Some(du_bytes(&selected)));
    assert!(retained.join("payload.bin").is_file());
    assert!(selected.join("payload.bin").is_file());
}

#[test]
fn the_human_report_retains_the_actual_pass_refusals_and_exhausted_limits() {
    let host = Host::new();
    host.seed_tree(&host.cache_root, "tagged-cache", 4, true);
    let result = host.run(&["disk-cleanup", "--once"]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let report = host.json(&["space", "report", TARGET, "--json"]);
    let recorded: Value =
        serde_json::from_slice(&fs::read(host.under_home(crate::fixture::JANITOR_STATE)).unwrap())
            .unwrap();
    assert_eq!(report["cleanup_state"]["report"], recorded["report"]);
    assert_eq!(report["coverage"]["janitor"]["report"], recorded["report"]);
    assert!(!host.cache_root.join("tagged-cache").exists());
    let human = host.run(&["space", "report", TARGET]);
    assert!(human.status.success());
    let text = String::from_utf8(human.stdout).unwrap();
    assert!(text.contains("cleaner build_caches:"));
}

/// A janitor pass that frees nothing still has to say what it looked at.
///
/// `registry_cleanup` is the only reclaim stage whose candidates come from the
/// host's declared cleaner policy, so its row is the only place an operator
/// can read what that policy covered. On `ubuntu-server-rtx-pro-6000` on
/// 2026-09-10 it answered `items: 0`, `detail: null` and carried no janitor
/// section at all - the same receipt a clean host produces. The scope here
/// holds one untagged tree, which is a real candidate the cleaner examines and
/// refuses, so the pass frees nothing and the receipt must still carry the
/// janitor's own numbers.
#[test]
fn a_reclaim_pass_that_frees_nothing_still_carries_the_janitors_counts() {
    let host = Host::new();
    let untagged = host.seed_tree(&host.cache_root, "untagged-tree", 4, false);
    let report = host.json(&[
        "space",
        "reclaim",
        TARGET,
        "--stage",
        "registry_cleanup",
        "--apply",
        "--reason",
        "space area: proving a pass that frees nothing reports its scope",
        "--json",
    ]);
    let stage = report["stages"]
        .as_array()
        .and_then(|stages| stages.iter().find(|row| row["stage"] == "registry_cleanup"))
        .expect("the selected stage is missing from the receipt")
        .clone();
    assert_eq!(stage["items"].as_u64(), Some(0));
    assert!(
        untagged.join("payload.bin").is_file(),
        "the pass deleted a tree no build tool tagged as regenerable"
    );

    let janitor = &report["janitor"]["cleaners"]["build_caches"];
    let scanned = janitor["scanned_items"]
        .as_i64()
        .expect("the receipt carries the janitor's own report");
    assert!(scanned >= 1, "the janitor scanned nothing: {janitor}");
    assert_eq!(janitor["eligible_items"].as_i64(), Some(0));
    assert_eq!(janitor["deleted_items"].as_i64(), Some(0));
    let detail = stage["detail"]
        .as_str()
        .expect("a stage that freed nothing must say why");
    assert!(
        detail.contains(&format!(
            "build_caches scanned {scanned} eligible 0 deleted 0"
        )),
        "the stage row does not carry the janitor's counts: {detail}"
    );
}

#[test]
fn a_backup_override_cannot_treat_the_primary_as_a_second_copy() {
    let host = Host::new();
    let primary = host.home.join(".stado/local-storage");
    let object = primary.join("ecosystem/replica-identity/only-copy.bin");
    fs::create_dir_all(object.parent().unwrap()).unwrap();
    fs::write(&object, b"the only primary copy").unwrap();
    let mut policy: Value = serde_json::from_str(&host.policy()).unwrap();
    policy["cleaners"] = serde_json::json!({});
    policy["cleaners"]["backup_twins"] = serde_json::json!({});
    policy["cleaners"]["backup_twins"]["min_age_seconds"] = serde_json::json!(0);
    policy["cleaners"]["backup_twins"]["root"] = serde_json::json!(primary);
    host.declare(&policy.to_string());
    let result = host.run(&["disk-cleanup", "--once"]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(
        fs::read(&object).expect("the primary is not a replica"),
        b"the only primary copy"
    );
}
