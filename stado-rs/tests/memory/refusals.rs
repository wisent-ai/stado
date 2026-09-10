//! The memory declaration refusals and the restart repair.
use super::*;

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
