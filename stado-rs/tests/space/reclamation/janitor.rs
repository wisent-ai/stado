//! The janitor stage: the declared cache it sweeps, the tag that decides, and
//! the home a relative root is read against.

use std::fs;

use serde_json::Value;

use crate::fixture::{only_stage, reported_paths, Host, JANITOR_STATE, TARGET};
use crate::system::{allocated_bytes, du_bytes, said};

use super::{assert_inside, CACHE_MIB};

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

/// The stage the janitor's cleaner cannot stand in for: the same tagged trees,
/// evicted by the command the operator runs.
///
/// On a Mac the janitor's `build_caches` cleaner is rooted inside the
/// operator's Documents folder and the agent that runs it holds no Full Disk
/// Access grant, so every pass ends `OSError (Operation not permitted)` having
/// freed nothing while the host publishes `disk_pressure_active`. This stage
/// is that eviction, run from the CLI. What it takes is exactly what carries a
/// build tool's own `CACHEDIR.TAG`; an untagged neighbour in the same root is
/// proof it is the tag and not the location that decides.
#[test]
fn the_tagged_stage_takes_the_tagged_tree_and_leaves_its_untagged_neighbour() {
    let host = Host::new();
    let cache_root = host.cache_root.clone();
    let tagged = host.seed_tree(&cache_root, "target", CACHE_MIB, true);
    let untagged = host.seed_tree(&cache_root, "sources", SCRATCH_MIB, false);

    let preview = host.json(&[
        "space",
        "reclaim",
        TARGET,
        "--stage",
        "tagged_build_caches",
        "--dry-run",
        "--json",
    ]);
    let paths = reported_paths(only_stage(&preview, "tagged_build_caches"));
    assert_inside(&host.root, &paths);
    assert_eq!(paths, vec![tagged.to_string_lossy().to_string()]);
    assert!(
        tagged.join("payload.bin").is_file(),
        "the preview deleted the payload"
    );

    let reason = "space area: proving the tagged stage evicts what the janitor cannot reach";
    let report = host.json(&[
        "space",
        "reclaim",
        TARGET,
        "--stage",
        "tagged_build_caches",
        "--apply",
        "--reason",
        reason,
        "--json",
    ]);
    let stage = only_stage(&report, "tagged_build_caches");
    assert_eq!(stage["items"].as_u64(), Some(1));
    assert!(!tagged.exists(), "the applied stage left the tagged tree");
    assert!(
        untagged.join("payload.bin").is_file(),
        "the applied stage took a directory no build tool tagged"
    );
    assert!(
        cache_root.is_dir(),
        "the applied stage removed the declared root itself"
    );

    let audit = fs::read_to_string(host.under_home(AUDIT_LOG))
        .expect("the applied run recorded itself here");
    assert!(
        audit.contains(reason),
        "the audit record does not carry the reason: {audit}"
    );
}

/// A cleaner root is declared the way an operator writes a path, and
/// `~/Documents/CodingProjects/Wisent` is how this fleet's Mac declares its
/// own. Quoted whole it is a directory that does not exist: the first run of
/// this stage against that host reported zero items while the machine held
/// 95.4 GiB of tagged build output and refused every queued job.
#[test]
fn a_home_relative_declared_root_is_swept_as_the_home_it_names() {
    let host = Host::new();
    let relative = host
        .cache_root
        .strip_prefix(&host.home)
        .expect("the fixture's cache root is inside its home")
        .to_string_lossy()
        .to_string();
    host.declare(&format!(
        r#"{{
        "mode": "enforce",
        "check_interval_seconds": 3600,
        "low_free_gb": 3999999,
        "target_free_gb": 4000000,
        "max_bytes_per_pass": 1073741824,
        "max_items_per_pass": 32,
        "max_scan_items": 4096,
        "cleaners": {{"build_caches": {{"min_age_seconds": 86400, "root": "~/{relative}"}}}}
      }}"#
    ));
    let tagged = host.seed_tree(&host.cache_root.clone(), "target", CACHE_MIB, true);

    let preview = host.json(&[
        "space",
        "reclaim",
        TARGET,
        "--stage",
        "tagged_build_caches",
        "--dry-run",
        "--json",
    ]);
    let paths = reported_paths(only_stage(&preview, "tagged_build_caches"));
    assert_inside(&host.root, &paths);
    assert_eq!(
        paths,
        vec![tagged.to_string_lossy().to_string()],
        "a root declared as ~/… found nothing; the tilde reached the filesystem unexpanded"
    );
}
