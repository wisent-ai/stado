//! Declared host-memory management, driven through the real `stado` binary
//! against an isolated registry and store.
//!
//! Every test runs the product's own commands — `stado space watermark` to
//! declare, `stado disk-cleanup --once` to execute the janitor's two passes,
//! `stado space watermark --json` to read back — with the product's own
//! backend variables pointing at a temporary directory. Nothing reaches the
//! fleet and nothing on this machine is repaired: a fixture either keeps the
//! host under its watermark, or declares a repair whose subject does not
//! exist here, so the pass reaches the repair, records a verdict, and changes
//! nothing.

mod constants;
mod harness;
mod policies;
mod policy_refusals;

use constants::{
    ALWAYS_OVER_LOW_MB, ALWAYS_OVER_TARGET_MB, INCOHERENT_TARGET_MB, NEVER_OVER_LOW_MB,
    NEVER_OVER_TARGET_MB, UNREACHABLE_SWAP_PCT,
};
use harness::{declare, registry_bytes, run_pass, setup, stado, stderr, ABSENT_UNIT, TARGET};

#[test]
fn a_declared_watermark_is_written_and_read_back() {
    let storage = setup();
    let written = declare(storage.path(), &["--memory-mode", "report", "--json"]);
    assert!(
        written.status.success(),
        "declaring the watermark failed: {}",
        stderr(&written)
    );

    let read = stado(storage.path(), &["space", "watermark", TARGET, "--json"]);
    assert!(read.status.success(), "read-back failed: {}", stderr(&read));
    let document: serde_json::Value = serde_json::from_slice(&read.stdout).unwrap();
    assert_eq!(document["declared"], serde_json::json!(true));
    let policy = &document["memory_reclaim"];
    assert_eq!(policy["mode"], serde_json::json!("report"));
    assert_eq!(policy["low_free_mb"], serde_json::json!(ALWAYS_OVER_LOW_MB));
    assert_eq!(
        policy["target_free_mb"],
        serde_json::json!(ALWAYS_OVER_TARGET_MB)
    );
    assert_eq!(
        policy["high_swap_used_pct"],
        serde_json::json!(UNREACHABLE_SWAP_PCT)
    );
}

#[test]
fn an_incoherent_declaration_is_refused_with_its_own_sentence() {
    let storage = setup();
    let before = registry_bytes(storage.path());

    let low = ALWAYS_OVER_LOW_MB.to_string();
    let target = INCOHERENT_TARGET_MB.to_string();
    let refused = stado(
        storage.path(),
        &[
            "space",
            "watermark",
            TARGET,
            "--memory-mode",
            "report",
            "--memory-low-free-mb",
            &low,
            "--memory-target-free-mb",
            &target,
        ],
    );
    assert!(
        !refused.status.success(),
        "an incoherent target watermark was accepted"
    );
    assert!(
        stderr(&refused).contains(
            "registry.targets[0].memory_reclaim.target_free_mb: must be greater than low_free_mb"
        ),
        "refusal did not carry its own sentence: {}",
        stderr(&refused)
    );

    let armed_with_nothing = stado(
        storage.path(),
        &["space", "watermark", TARGET, "--memory-mode", "enforce"],
    );
    assert!(
        !armed_with_nothing.status.success(),
        "enforce with no declared repair was accepted"
    );
    assert!(
        stderr(&armed_with_nothing)
            .contains("must name at least one repair when mode is 'enforce'"),
        "refusal did not carry its own sentence: {}",
        stderr(&armed_with_nothing)
    );

    assert_eq!(
        before,
        registry_bytes(storage.path()),
        "a refused declaration still changed the canonical registry"
    );
}

#[test]
fn a_host_over_its_watermark_is_refused_for_placement() {
    let storage = setup();
    let declared = declare(
        storage.path(),
        &[
            "--memory-mode",
            "report",
            "--memory-refuse-placement",
            "true",
        ],
    );
    assert!(
        declared.status.success(),
        "declaring the refusal failed: {}",
        stderr(&declared)
    );

    let report = run_pass(storage.path());
    assert_eq!(report["pressure_active"], serde_json::json!(true));
    assert_eq!(report["refuse_placement"], serde_json::json!(true));
    assert_eq!(
        report["placement_refusal"],
        serde_json::json!("memory_pressure_active"),
        "the pass did not publish the admission reason: {report}"
    );
}

/// Swap over its watermark on a host that still has its memory headroom is a
/// finding, not a refusal.
///
/// On 2026-09-10 `ubuntu-server-rtx-pro-6000` held 67.2 GB available of 132.1
/// GB against an 8 GiB watermark with 85% of an 8.59 GB swap file in use, so
/// it refused every job and `skarbiec` could not build `linux-amd64` in three
/// consecutive releases. Withholding a host with 64 GiB of headroom frees no
/// memory; it only removes the fleet's one Linux builder.
#[test]
fn used_swap_alone_does_not_withhold_a_host_with_memory_headroom() {
    let storage = setup();
    let low = NEVER_OVER_LOW_MB.to_string();
    let target = NEVER_OVER_TARGET_MB.to_string();
    let declared = stado(
        storage.path(),
        &[
            "space",
            "watermark",
            TARGET,
            "--memory-mode",
            "report",
            "--memory-low-free-mb",
            &low,
            "--memory-target-free-mb",
            &target,
            // Every host with any swap in use is over this watermark.
            "--memory-high-swap-used-pct",
            "1",
            "--memory-refuse-placement",
            "true",
        ],
    );
    assert!(
        declared.status.success(),
        "declaring the swap watermark failed: {}",
        stderr(&declared)
    );

    let report = run_pass(storage.path());
    let reading = &report["memory_before"];
    assert!(
        reading["available_bytes"].is_i64(),
        "the pass reported no memory reading: {report}"
    );
    assert_eq!(
        report["placement_refusal"],
        serde_json::Value::Null,
        "a host with its memory headroom was withheld from placement: {report}"
    );
    let swap_used = reading["swap_used_bytes"].as_i64();
    if swap_used.is_some_and(|used| used > i64::default()) {
        assert_eq!(
            report["pressure_active"],
            serde_json::json!(true),
            "swap over its watermark was not reported at all: {report}"
        );
    }
}

#[test]
fn a_declared_unit_restart_is_attempted_and_recorded() {
    let storage = setup();
    let declared = declare(
        storage.path(),
        &[
            "--memory-mode",
            "enforce",
            "--memory-repair",
            "restart_unit",
            "--memory-repair-unit",
            ABSENT_UNIT,
        ],
    );
    assert!(
        declared.status.success(),
        "declaring the repair failed: {}",
        stderr(&declared)
    );

    let report = run_pass(storage.path());
    assert_eq!(report["mode"], serde_json::json!("enforce"));
    assert_eq!(report["pressure_active"], serde_json::json!(true));
    let repair = &report["repairs"]["restart_unit"];
    assert_eq!(
        repair["subjects"],
        serde_json::json!([ABSENT_UNIT]),
        "the pass did not record the declared subject: {report}"
    );
    assert_eq!(
        repair["examined"],
        serde_json::json!(1),
        "the pass did not reach the declared repair: {report}"
    );
    assert_eq!(
        repair["skipped"]["unit_not_loaded"],
        serde_json::json!(1),
        "the pass did not record why it did not restart: {report}"
    );
    assert_eq!(repair["repaired"], serde_json::json!(0));
    assert_eq!(report["outcome"], serde_json::json!("no_eligible_items"));
}

#[test]
fn a_host_that_declares_nothing_reports_and_changes_nothing() {
    let storage = setup();
    let before = registry_bytes(storage.path());

    let read = stado(storage.path(), &["space", "watermark", TARGET, "--json"]);
    assert!(read.status.success(), "read failed: {}", stderr(&read));
    let document: serde_json::Value = serde_json::from_slice(&read.stdout).unwrap();
    assert_eq!(document["declared"], serde_json::json!(false));

    let report = run_pass(storage.path());
    assert_eq!(
        report["policy_defaulted"],
        serde_json::json!(true),
        "an undeclared host did not say so: {report}"
    );
    assert_eq!(report["mode"], serde_json::json!("report"));
    assert_eq!(report["refuse_placement"], serde_json::json!(false));
    assert_eq!(report["placement_refusal"], serde_json::Value::Null);
    assert!(
        report["memory_before"]["available_bytes"].is_i64(),
        "the reporting default reported no memory at all: {report}"
    );
    let repaired = report["repairs"]
        .as_object()
        .map(|repairs| {
            repairs
                .values()
                .filter_map(|entry| entry["repaired"].as_i64())
                .sum::<i64>()
        })
        .unwrap_or_default();
    assert_eq!(repaired, 0, "the reporting default repaired something");
    assert_eq!(
        before,
        registry_bytes(storage.path()),
        "a reporting pass rewrote the canonical registry"
    );
}

#[test]
fn a_host_under_its_watermark_is_healthy() {
    let storage = setup();
    let low = NEVER_OVER_LOW_MB.to_string();
    let target = NEVER_OVER_TARGET_MB.to_string();
    let swap = UNREACHABLE_SWAP_PCT.to_string();
    let declared = stado(
        storage.path(),
        &[
            "space",
            "watermark",
            TARGET,
            "--memory-mode",
            "report",
            "--memory-low-free-mb",
            &low,
            "--memory-target-free-mb",
            &target,
            "--memory-high-swap-used-pct",
            &swap,
        ],
    );
    assert!(
        declared.status.success(),
        "declaring the watermark failed: {}",
        stderr(&declared)
    );
    let report = run_pass(storage.path());
    assert_eq!(report["pressure_active"], serde_json::json!(false));
    assert_eq!(report["outcome"], serde_json::json!("healthy_noop"));
    assert_eq!(report["repairs"], serde_json::Value::Null);
}
