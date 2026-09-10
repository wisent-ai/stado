//! `stado space` against a target this area leases for itself.
//!
//! The cases beside this one drive the capability against the machine running
//! the test, which is the only honest way to read a filesystem this process
//! can see. What they cannot do is change one: nothing may delete an
//! operator's bytes to prove that reclamation reclaims, and a tempdir standing
//! in for a host proves nothing about a host. These cases take a disposable
//! account on a registry host through `stado scratch`, declared in the
//! registry that lease emits, write and reclaim inside that account's own
//! home, and destroy the lease when they are done.
//!
//! Every figure asserted here is read back off the leased machine through a
//! path that is not the command's own report: a second `stado space report`
//! for the bytes and the inventory, and the allowlisted `stado host exec` `df`
//! probe, a different channel from the reporting script, for the volume.
//! Nothing is skipped: a fleet that will not lease fails the case.

mod fleet;

use serde_json::{json, Value};

use fleet::{said, text, Lease, AUDIT_LOG, AUDIT_ROOT, BUILD_WORK_ROOT, JANITOR_STATE, KIB};

/// The reason the applying case gives, recorded on the host whose disk changed.
const REASON: &str = "space area: proving a leased host gives the bytes back";
/// The gibibytes the inventory must attribute to the seeded tree while it is
/// there. The payload is a quarter of one and the inventory reports tenths.
const SEEDED_GB: f64 = 0.2;
/// `df -Pk` capacity is a whole percentage and the check below divides the
/// other way, so the two may differ by one point.
const CAPACITY_DRIFT: i64 = 1;
/// `CmdError::click` — a refusal about the fleet's own state.
const CLICK_EXIT: i32 = 1;
const PERCENT: i64 = 100;

/// One `df` field of the report, in 1024-byte blocks.
fn blocks(usage: &Value, field: &str) -> i64 {
    usage[field]
        .as_str()
        .unwrap_or_else(|| panic!("usage.{field} is a string of blocks: {usage}"))
        .parse()
        .unwrap_or_else(|exc| panic!("usage.{field} is not a block count: {exc}"))
}

/// The gibibytes the report's own inventory attributes to one path, or `None`
/// when the host's `du` no longer names it at all.
fn inventory_gb(report: &Value, path: &str) -> Option<f64> {
    report["inventory"]
        .as_array()
        .expect("the report carries an inventory")
        .iter()
        .find(|row| row["path"].as_str() == Some(path))
        .and_then(|row| row["size_gb"].as_f64())
}

/// The one stage a `--stage build_scratch` selection ran.
fn only_stage(report: &Value) -> &Value {
    let stages = report["stages"]
        .as_array()
        .expect("the reclamation reports its stages");
    assert_eq!(stages.len(), 1, "expected one stage, got {stages:?}");
    assert_eq!(stages[0]["stage"], "build_scratch");
    &stages[0]
}

/// The report is the leased machine's own filesystem, memory and swap, and the
/// host's own `df`, read back through the allowlisted probe, names the same
/// volume. Nothing here is pinned to a figure: what is asserted is that the
/// numbers describe one real machine and agree with each other.
#[test]
fn a_leased_targets_report_is_that_machines_own_space() {
    let mut lease = Lease::take();
    lease.declare_cleanup();
    let report = lease.json(&["space", "report", &lease.name, "--json"]);
    assert_eq!(report["target"], Value::from(lease.name.clone()));

    let usage = &report["usage"];
    assert_eq!(usage["mounted_on"], "/");
    let filesystem = text(usage, "filesystem");
    let total = blocks(usage, "blocks_kb");
    let used = blocks(usage, "used_kb");
    let free = blocks(usage, "available_kb");
    assert!(
        used > 0 && free > 0 && used + free <= total,
        "the reported figures are not a real filesystem: {usage}"
    );
    let capacity: i64 = text(usage, "capacity")
        .trim_end_matches('%')
        .parse()
        .unwrap_or_else(|exc| panic!("capacity is not a df percentage: {exc}"));
    assert!(
        (capacity - (used * PERCENT) / (used + free)).abs() <= CAPACITY_DRIFT,
        "capacity {capacity}% does not describe {used} KiB used and {free} KiB available"
    );
    assert_eq!(
        report["free_space"]["available_bytes"].as_i64(),
        Some(free * KIB),
        "the watermark section and the row above it disagree about free space"
    );

    let memory = &report["memory"];
    let free_kb: i64 = text(memory, "free_kb")
        .parse()
        .unwrap_or_else(|exc| panic!("memory.free_kb is not a block count: {exc}"));
    assert!(free_kb > 0, "the leased machine reported no free memory");
    let swap = text(memory, "swap");
    assert!(
        swap.contains("total =") && swap.contains("used =") && swap.contains("free ="),
        "the swap line is not that machine's own sysctl answer: {swap}"
    );
    let reading = &report["memory_reclaim"]["reading"];
    let installed = reading["total_bytes"].as_i64().expect("installed memory");
    let spare = reading["available_bytes"]
        .as_i64()
        .expect("available memory");
    let swap_total = reading["swap_total_bytes"].as_i64().expect("swap total");
    let swap_used = reading["swap_used_bytes"].as_i64().expect("swap used");
    assert!(
        installed > 0 && spare > 0 && spare < installed,
        "{spare} bytes available of {installed} installed is not a live memory reading"
    );
    assert!(
        swap_total >= 0 && swap_used >= 0 && swap_used <= swap_total,
        "{swap_used} bytes used of {swap_total} is not a valid swap reading"
    );

    // Whose machine it is: the paths the report names are inside the leased
    // account's home, which does not exist on the machine running the test.
    assert_eq!(
        report["cleanup_state"]["path"],
        Value::from(lease.under_home(JANITOR_STATE))
    );
    assert!(
        report["inventory"]
            .as_array()
            .expect("an inventory")
            .iter()
            .any(|row| row["path"]
                .as_str()
                .is_some_and(|path| path.starts_with(&lease.home))),
        "the inventory named nothing inside the leased account's home: {report}"
    );

    let probe = lease.run(&["host", "exec", &lease.name, "--", "df", "-h"]);
    assert!(
        probe.status.success(),
        "the allowlisted df probe did not run on the leased target: {}",
        said(&probe.stderr)
    );
    let printed = said(&probe.stdout);
    let row = printed
        .lines()
        .find(|line| line.split_whitespace().last() == Some("/"))
        .unwrap_or_else(|| panic!("the host's own df named no root filesystem: {printed}"))
        .to_string();
    assert!(
        row.starts_with(&filesystem),
        "the report named {filesystem}; the host's own df says {row}"
    );

    lease.destroy();
}

/// Verify the leased account's own inventory and audit after preview and apply.
/// Whole-filesystem free space is not isolated from concurrent fleet activity.
#[test]
fn applying_the_scratch_stage_frees_the_bytes_it_named_on_the_leased_host() {
    let mut lease = Lease::take();
    lease.declare_cleanup();
    let read = ["space", "report", &lease.name, "--json"];

    let tree = lease.seed_scratch();
    let scratch_root = lease.under_home(BUILD_WORK_ROOT);
    let audit_root = lease.under_home(AUDIT_ROOT);
    let seeded_report = lease.json(&read);
    assert!(
        inventory_gb(&seeded_report, &scratch_root).unwrap_or_default() >= SEEDED_GB,
        "the report's own inventory does not hold the payload: {seeded_report}"
    );
    assert!(
        inventory_gb(&seeded_report, &audit_root).is_none(),
        "the leased account already holds a reclamation record"
    );

    let preview = lease.json(&[
        "space",
        "reclaim",
        &lease.name,
        "--stage",
        "build_scratch",
        "--dry-run",
        "--json",
    ]);
    assert_eq!(preview["mode"], "dry_run");
    assert_eq!(preview["selected_stages"], json!(["build_scratch"]));
    assert_eq!(only_stage(&preview)["items"].as_u64(), Some(1));
    assert_eq!(only_stage(&preview)["paths"], json!([tree]));
    assert_eq!(preview["audit_log"], Value::Null);
    let previewed = lease.json(&read);
    assert!(
        inventory_gb(&previewed, &scratch_root).unwrap_or_default() >= SEEDED_GB,
        "the preview removed the tree it was only asked to name"
    );

    let applied = lease.json(&[
        "space",
        "reclaim",
        &lease.name,
        "--stage",
        "build_scratch",
        "--apply",
        "--reason",
        REASON,
        "--json",
    ]);
    assert_eq!(applied["mode"], "apply");
    assert_eq!(only_stage(&applied)["items"].as_u64(), Some(1));
    assert_eq!(only_stage(&applied)["paths"], json!([tree]));
    assert_eq!(
        applied["audit_log"],
        Value::from(lease.under_home(AUDIT_LOG)),
        "the applied run recorded itself somewhere other than the host it changed"
    );

    let after = lease.json(&read);
    assert_eq!(
        inventory_gb(&after, &scratch_root).unwrap_or_default(),
        0.0,
        "the leased host still holds the tree the stage said it removed"
    );
    assert!(
        inventory_gb(&after, &audit_root).is_some(),
        "the applied run left no record in the leased account's own home"
    );

    lease.destroy();
}

/// A lease's registry as it is emitted declares no cleanup policy, so the APFS
/// snapshot stage has no target watermark to work against, reports itself
/// unavailable rather than pretending to have swept, and the capability
/// refuses the whole run with its own sentence.
#[test]
fn a_leased_target_with_no_eligible_stage_is_refused_by_its_own_sentence() {
    let mut lease = Lease::take();
    let refused = lease.run(&[
        "space",
        "reclaim",
        &lease.name,
        "--stage",
        "local_apfs_snapshots",
        "--dry-run",
        "--json",
    ]);
    assert_eq!(
        refused.status.code(),
        Some(CLICK_EXIT),
        "the refusal did not leave through the fleet-state exit: {}",
        said(&refused.stderr)
    );
    let sentence = format!(
        "{} declares no eligible space reclamation stage; \
         add it to stado-rs/data/fleet/space.json reclaim_stages",
        lease.name
    );
    let printed: Value = serde_json::from_slice(&refused.stdout)
        .unwrap_or_else(|exc| panic!("the refusal printed no JSON document: {exc}"));
    assert_eq!(printed["message"], Value::from(sentence.clone()));
    assert_eq!(printed["failure_point"], "cli.space.reclaim");
    assert!(
        said(&refused.stderr).contains(&sentence),
        "the refusal did not say it on stderr: {}",
        said(&refused.stderr)
    );

    // The refusal left the machine alone: with a policy declared afterwards,
    // the leased host reports no janitor state and no reclamation record.
    lease.declare_cleanup();
    let report = lease.json(&["space", "report", &lease.name, "--json"]);
    assert_eq!(report["cleanup_state"]["present"], false);
    assert!(
        inventory_gb(&report, &lease.under_home(AUDIT_ROOT)).is_none(),
        "a refused reclamation recorded itself on the leased host: {report}"
    );

    lease.destroy();
}
