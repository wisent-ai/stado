//! Reclamation that belongs to the host, not to the queue.
//!
//! # What went wrong
//!
//! `charless-mac-mini` held 34.9 GiB of a product's run evidence in its
//! object store, sat at 3.7 GiB free against an 8 GiB watermark, and
//! published `not accepting jobs: disk_pressure_active`. The product owns
//! how long that evidence is kept and has a command that expires it — and
//! the job carrying that command could never be claimed, because the host
//! refuses work while it is under the pressure that work would relieve.
//!
//! # What is defended here
//!
//! The cleaner that moves the decision to the host's own janitor, against a
//! real tree with real file ages: a declaration with no root sweeps nothing
//! rather than guessing a path, a planning pass counts what is eligible and
//! deletes none of it, an enforcing pass removes only what is past the
//! declared age, and a spent scan budget stops the pass and says so.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use stado::providers::local::disk_cleanup::object_evidence::{scan_object_evidence, CLEANER};
use stado::providers::local::disk_cleanup::CleanupReport;
use stado::targets::{DiskCleanerPolicy, DiskCleanupPolicy};

/// A week, the retention floor this cleaner's declaration carries.
const MIN_AGE_SECONDS: i64 = 604_800;
/// Enough scan budget that these few files are never the bound, except in
/// the case that sets its own.
const SCAN_BUDGET: i64 = 1_000;
const DAY_SECONDS: u64 = 86_400;
/// The rest of the pass policy, copied from what a fleet Mac declares: the
/// janitor's interval and watermarks, and no per-pass byte or item cap, so
/// the only limits in play are the ones each case is about.
const CHECK_INTERVAL_SECONDS: i64 = 300;
const LOW_FREE_GB: i64 = 20;
const TARGET_FREE_GB: i64 = 30;
const NO_BYTE_CAP: i64 = 0;
const NO_ITEM_CAP: i64 = 0;
const PASS_SECONDS: i64 = 600;
/// The deadline every case runs under; the budget case bounds itself by
/// items instead.
const CASE_DEADLINE_SECONDS: u64 = 60;

fn scratch(name: &str) -> PathBuf {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("object-evidence-tests")
        .join(name);
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).expect("a scratch root");
    root
}

fn write_aged(path: &Path, bytes: usize, age_days: u64) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("the parent of an evidence file");
    }
    fs::write(path, vec![b'e'; bytes]).expect("an evidence file");
    let when = SystemTime::now() - Duration::from_secs(age_days * DAY_SECONDS);
    let times = fs::FileTimes::new().set_modified(when);
    fs::File::options()
        .write(true)
        .open(path)
        .expect("the file to stamp")
        .set_times(times)
        .expect("stamping the file");
}

fn policy(root: Option<&str>) -> DiskCleanupPolicy {
    let mut cleaners = BTreeMap::new();
    cleaners.insert(
        CLEANER.to_string(),
        DiskCleanerPolicy {
            min_age_seconds: MIN_AGE_SECONDS,
            allow_missing_upload_proof: false,
            root: root.map(str::to_string),
            keep_newest: None,
        },
    );
    DiskCleanupPolicy {
        mode: "enforce".to_string(),
        check_interval_seconds: CHECK_INTERVAL_SECONDS,
        low_free_gb: LOW_FREE_GB,
        target_free_gb: TARGET_FREE_GB,
        max_bytes_per_pass: NO_BYTE_CAP,
        max_items_per_pass: NO_ITEM_CAP,
        max_scan_items: SCAN_BUDGET,
        max_pass_seconds: Some(PASS_SECONDS),
        cleaners,
    }
}

fn now_seconds() -> f64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .expect("a clock after the epoch")
        .as_secs_f64()
}

fn run(policy: &DiskCleanupPolicy, home: &Path, enforcing: bool, budget: i64) -> CleanupReport {
    let mut report = CleanupReport::base(0, "object-evidence-test");
    scan_object_evidence(
        home,
        policy,
        now_seconds(),
        budget,
        Instant::now() + Duration::from_secs(CASE_DEADLINE_SECONDS),
        enforcing,
        &mut report,
    );
    report
}

#[test]
fn a_declaration_without_a_root_sweeps_nothing() {
    let home = scratch("no-root");
    let report = run(&policy(None), &home, true, SCAN_BUDGET);
    assert_eq!(report.object_evidence.scanned_items, 0);
    assert_eq!(
        report.object_evidence.skipped.get("root_undeclared"),
        Some(&1),
        "a cleaner with no declared root must say so: {:?}",
        report.object_evidence.skipped
    );
}

#[test]
fn a_planning_pass_counts_what_is_old_and_deletes_none_of_it() {
    let home = scratch("planning");
    let root = home.join("store/ecosystem/product/results");
    write_aged(&root.join("old-run.tar.gz"), 4_096, 30);
    write_aged(&root.join("fresh-run.tar.gz"), 2_048, 1);

    let report = run(
        &policy(Some(root.to_str().expect("a path"))),
        &home,
        false,
        SCAN_BUDGET,
    );
    assert_eq!(
        report.object_evidence.eligible_items, 1,
        "one file is past the week"
    );
    assert_eq!(report.object_evidence.expected_bytes, 4_096);
    assert_eq!(
        report.object_evidence.deleted_items, 0,
        "a plan deletes nothing"
    );
    assert!(
        root.join("old-run.tar.gz").exists(),
        "the plan left the file alone"
    );
    assert_eq!(
        report.object_evidence.skipped.get("younger_than_min_age"),
        Some(&1)
    );
}

/// The fleet's pinned build inputs live under the same `artifacts/` tree as
/// run evidence, are addressed by their own digest, and never expire. A pass
/// over that tree on 2026-09-21 deleted the Apple issuer chain and the
/// pinned signer, and the next darwin release died in `macos-code-signing`
/// with `cannot read native signing input ... apple-issuers-<sha>.pem`.
#[test]
fn a_pinned_signing_input_is_kept_however_old_it_is() {
    let home = scratch("pinned");
    let root = home.join("store/ecosystem/product/artifacts");
    write_aged(
        &root.join("native-signing/apple-issuers-deadbeef.pem"),
        2_048,
        400,
    );
    write_aged(&root.join("native-signing/6a2781e2.tar.gz"), 4_096, 400);
    write_aged(&root.join("evidence/old-run.tar.gz"), 1_024, 30);

    let report = run(
        &policy(Some(root.to_str().expect("a path"))),
        &home,
        true,
        SCAN_BUDGET,
    );
    assert_eq!(
        report.object_evidence.skipped.get("pinned_input_kept"),
        Some(&2),
        "both pinned inputs are kept and counted: {:?}",
        report.object_evidence
    );
    assert!(root
        .join("native-signing/apple-issuers-deadbeef.pem")
        .exists());
    assert!(root.join("native-signing/6a2781e2.tar.gz").exists());
    assert_eq!(
        report.object_evidence.deleted_items, 1,
        "the run evidence beside them still expires"
    );
    assert!(!root.join("evidence/old-run.tar.gz").exists());
}

#[test]
fn an_enforcing_pass_removes_only_what_is_past_the_declared_age() {
    let home = scratch("enforcing");
    let root = home.join("store/ecosystem/product/results");
    write_aged(&root.join("old-run.tar.gz"), 4_096, 30);
    write_aged(&root.join("nested/older-run.log"), 1_024, 60);
    write_aged(&root.join("fresh-run.tar.gz"), 2_048, 1);

    let report = run(
        &policy(Some(root.to_str().expect("a path"))),
        &home,
        true,
        SCAN_BUDGET,
    );
    assert_eq!(
        report.object_evidence.deleted_items, 2,
        "{:?}",
        report.object_evidence
    );
    assert_eq!(report.object_evidence.actual_free_delta_bytes, 5_120);
    assert!(!root.join("old-run.tar.gz").exists());
    assert!(!root.join("nested/older-run.log").exists());
    assert!(
        root.join("fresh-run.tar.gz").exists(),
        "evidence inside the declared window stays"
    );
}

#[test]
fn a_spent_scan_budget_stops_the_pass_and_is_reported() {
    let home = scratch("budget");
    let root = home.join("store/ecosystem/product/results");
    for index in 0..4 {
        write_aged(&root.join(format!("run-{index}.tar.gz")), 512, 30);
    }

    let report = run(
        &policy(Some(root.to_str().expect("a path"))),
        &home,
        true,
        2,
    );
    assert!(
        report.caps.scan,
        "the pass records that its budget bound it"
    );
    assert_eq!(report.object_evidence.skipped.get("scan_cap"), Some(&1));
    assert!(
        report.object_evidence.deleted_items <= 2,
        "a bounded pass never spends more than its budget: {:?}",
        report.object_evidence
    );
}
