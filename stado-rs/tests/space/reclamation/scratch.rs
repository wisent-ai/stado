//! The build scratch stage: what a preview names, and what applying removes.

use std::fs;

use serde_json::Value;

use crate::fixture::{only_stage, reported_paths, Host, AUDIT_LOG, BUILD_WORK_ROOT, TARGET};
use crate::system::{allocated_bytes, du_bytes, said};

use super::{assert_inside, SCRATCH_MIB};

/// A preview names the stale scratch tree and removes nothing.
#[test]
fn a_preview_names_the_scratch_tree_and_removes_nothing() {
    let host = Host::new();
    let scratch_root = host.under_home(BUILD_WORK_ROOT);
    fs::create_dir_all(&scratch_root).expect("create the build scratch root");
    let tree = host.seed_tree(&scratch_root, "release-tree", SCRATCH_MIB, false);
    let before = allocated_bytes(&tree);
    assert!(before > 0, "the fixture wrote no payload");

    let report = host.json(&[
        "space",
        "reclaim",
        TARGET,
        "--stage",
        "build_scratch",
        "--dry-run",
        "--json",
    ]);
    assert_eq!(report["mode"], "dry_run");
    assert_eq!(
        report["selected_stages"],
        serde_json::json!(["build_scratch"])
    );
    let stage = only_stage(&report, "build_scratch");
    let paths = reported_paths(stage);
    assert_inside(&host.root, &paths);
    assert_eq!(paths, vec![tree.to_string_lossy().to_string()]);

    assert!(
        tree.join("payload.bin").is_file(),
        "the preview deleted the payload"
    );
    assert_eq!(allocated_bytes(&tree), before);
    assert_eq!(report["audit_log"], Value::Null);
    assert!(
        !host.under_home(AUDIT_LOG).exists(),
        "a preview wrote an audit record"
    );
}

/// Applying removes the tree and records the reason on the host whose disk
/// changed.
#[test]
fn applying_removes_the_scratch_tree_and_records_the_reason_here() {
    let host = Host::new();
    let scratch_root = host.under_home(BUILD_WORK_ROOT);
    fs::create_dir_all(&scratch_root).expect("create the build scratch root");
    let tree = host.seed_tree(&scratch_root, "release-tree", SCRATCH_MIB, false);
    let payload = tree.join("payload.bin");
    assert!(payload.is_file(), "the fixture wrote no payload");

    let preview = host.json(&[
        "space",
        "reclaim",
        TARGET,
        "--stage",
        "build_scratch",
        "--dry-run",
        "--json",
    ]);
    assert_inside(
        &host.root,
        &reported_paths(only_stage(&preview, "build_scratch")),
    );

    let reason = "space area: proving the scratch stage removes what it names";
    let report = host.json(&[
        "space",
        "reclaim",
        TARGET,
        "--stage",
        "build_scratch",
        "--apply",
        "--reason",
        reason,
        "--json",
    ]);
    assert_eq!(report["mode"], "apply");
    let stage = only_stage(&report, "build_scratch");
    assert_eq!(stage["items"].as_u64(), Some(1));
    assert_eq!(
        reported_paths(stage),
        vec![tree.to_string_lossy().to_string()]
    );

    assert!(!tree.exists(), "the applied stage left the tree behind");
    assert!(
        !payload.exists(),
        "the applied stage left the payload behind"
    );
    assert!(
        scratch_root.is_dir(),
        "the applied stage removed the root it was only supposed to sweep"
    );

    let audit_path = host.under_home(AUDIT_LOG);
    assert_eq!(
        report["audit_log"].as_str(),
        Some(&*audit_path.to_string_lossy())
    );
    let audit = fs::read_to_string(&audit_path).expect("the applied run recorded itself here");
    let record: Value =
        serde_json::from_str(audit.trim()).expect("the audit log is one JSON-lines record");
    assert_eq!(record["reason"], reason);
    assert_eq!(record["command"], "stado space reclaim");
    assert_eq!(record["host"], TARGET);
    assert_eq!(record["mode"], "apply");
    assert_eq!(
        record["stages"][0]["paths"],
        serde_json::json!([tree.to_string_lossy()])
    );
}

/// The janitor stage removes the declared cache and charges exactly the bytes
