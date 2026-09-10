//! What a host's memory reading does to placement: a host over its
//! watermark is withheld, swap alone is not enough to withhold one that has
//! headroom, and a host under its watermark is healthy.

use crate::constants::{
    NEVER_OVER_LOW_MB, NEVER_OVER_TARGET_MB,
    UNREACHABLE_SWAP_PCT,
};
use crate::harness::{declare, run_pass, setup, stado, stderr, TARGET};

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
