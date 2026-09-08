//! Reclaiming space for real, in a scope the case created itself.
//!
//! Both applying cases select one stage by name, and both stages sweep only
//! roots below the fixture's `HOME`: `build_scratch` takes stale trees under
//! `$HOME/.stado/build-work`, and `registry_cleanup` runs this binary's own
//! janitor, whose single declared cleaner is rooted at the fixture's
//! build-cache directory. Before anything is applied the dry run's paths are
//! read and the case refuses to continue unless every one of them is inside
//! its own tempdir, so a stage that ever widened its enumeration fails the
//! test instead of deleting an operator's files.
//!
//! Removal is proved by reading the filesystem — the payload exists before and
//! is gone after — and the byte figure by the janitor's own state document,
//! which has to agree with `stat`'s allocated blocks and with `du -sk`.

use std::fs;
use std::path::Path;

use serde_json::Value;

use crate::fixture::{
    only_stage, reported_paths, Host, AUDIT_LOG, BUILD_WORK_ROOT, JANITOR_STATE, TARGET,
};
use crate::system::{allocated_bytes, du_bytes, said};

/// Payload sizes for the scopes these cases create inside their own tempdir.
/// Small enough to write quickly, large enough that `du` and `stat` report
/// several whole allocation blocks rather than a rounding artefact.
const SCRATCH_MIB: usize = 4;
const CACHE_MIB: usize = 8;

/// Refuse to apply anything unless every path the preview named is inside the
/// tempdir this case owns.
fn assert_inside(root: &Path, paths: &[String]) {
    let root = root.to_string_lossy().to_string();
    for path in paths {
        assert!(
            path.starts_with(&root),
            "the preview named {path}, which is outside this test's tempdir {root}; refusing to apply"
        );
    }
}

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
/// the operating system says that tree occupied.
#[test]
fn the_janitor_stage_removes_the_declared_cache_and_charges_its_bytes() {
    let host = Host::new();
    let cache_root = host.cache_root.clone();
    let tree = host.seed_tree(&cache_root, "target-tree", CACHE_MIB, true);
    let payload = tree.join("payload.bin");
    let allocated = allocated_bytes(&tree);
    let measured = du_bytes(&tree);
    assert!(allocated > 0, "the fixture wrote no payload");

    let reason = "space area: proving the janitor stage reclaims its declared root";
    let report = host.json(&[
        "space",
        "reclaim",
        TARGET,
        "--stage",
        "registry_cleanup",
        "--apply",
        "--reason",
        reason,
        "--json",
    ]);
    assert_eq!(report["mode"], "apply");
    let stage = only_stage(&report, "registry_cleanup");
    assert_eq!(stage["items"].as_u64(), Some(1));

    assert!(!tree.exists(), "the janitor left the tagged cache behind");
    assert!(!payload.exists(), "the janitor left the payload behind");
    assert!(
        cache_root.is_dir(),
        "the janitor removed the declared root itself"
    );

    let state_path = host.under_home(JANITOR_STATE);
    let state: Value = serde_json::from_str(
        &fs::read_to_string(&state_path).expect("the janitor pass wrote its state document"),
    )
    .expect("the janitor state is JSON");
    let cleaner = &state["report"]["cleaners"]["build_caches"];
    assert_eq!(cleaner["scanned_items"].as_i64(), Some(1));
    assert_eq!(cleaner["eligible_items"].as_i64(), Some(1));
    assert_eq!(cleaner["deleted_items"].as_i64(), Some(1));
    assert_eq!(
        cleaner["expected_bytes"].as_i64(),
        Some(allocated),
        "the janitor charged {} bytes; stat says the tree held {allocated}",
        cleaner["expected_bytes"]
    );
    assert!(
        measured >= allocated,
        "du reported {measured} bytes for a tree stat says held {allocated}"
    );
    assert!(
        state["report"]["free_bytes_after"]
            .as_i64()
            .expect("the pass measured free space after itself")
            > 0
    );

    // The same run has to be recorded on the host whose disk it changed.
    let audit = fs::read_to_string(host.under_home(AUDIT_LOG))
        .expect("the applied run recorded itself here");
    assert!(
        audit.contains(reason),
        "the audit record does not carry the reason: {audit}"
    );
}

/// A stage nobody asked for is never run: the janitor's own state proves the
/// pass never touched the declared cache when a different stage was selected.
#[test]
fn selecting_one_stage_leaves_the_other_scopes_alone() {
    let host = Host::new();
    let cache_root = host.cache_root.clone();
    let cache = host.seed_tree(&cache_root, "target-tree", CACHE_MIB, true);
    let scratch_root = host.under_home(BUILD_WORK_ROOT);
    fs::create_dir_all(&scratch_root).expect("create the build scratch root");
    let scratch = host.seed_tree(&scratch_root, "release-tree", SCRATCH_MIB, false);

    let output = host.run(&[
        "space",
        "reclaim",
        TARGET,
        "--stage",
        "build_scratch",
        "--apply",
        "--reason",
        "space area: proving stage selection is honoured",
        "--json",
    ]);
    assert!(output.status.success(), "{}", said(&output.stderr));

    assert!(!scratch.exists(), "the selected stage removed nothing");
    assert!(
        cache.join("payload.bin").is_file(),
        "a stage nobody selected reclaimed the declared cache"
    );
    assert!(
        !host.under_home(JANITOR_STATE).exists(),
        "the janitor ran for a stage nobody selected"
    );
}
