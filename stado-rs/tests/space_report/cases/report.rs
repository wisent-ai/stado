//! What the report says when the walk did not run: the cheap fields that cost
//! under a second, the walk it names as not attempted, and every volume.

use std::path::Path;

use crate::fixture::Fixture;

#[test]
fn a_report_without_the_walk_still_carries_free_space_memory_and_the_janitor() {
    let fixture = Fixture::new();
    let output = fixture.report("0", &[]);
    let text = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();

    assert_eq!(
        output.status.code(),
        Some(0),
        "a skipped walk is not a failed command; stderr: {stderr}"
    );
    assert!(
        text.contains("disk:") && text.contains("free:") && text.contains("GiB"),
        "the disk and free-space lines survive the missing walk: {text}"
    );
    assert!(
        text.contains("memory:"),
        "the memory reading survives the missing walk: {text}"
    );
    assert!(
        text.contains("janitor:"),
        "the janitor's own outcome survives the missing walk: {text}"
    );
    assert!(
        text.contains("inventory incomplete:"),
        "the missing inventory was hidden: {text}"
    );
    fixture.cleanup();
}

#[test]
fn the_report_names_the_walk_it_did_not_run() {
    let fixture = Fixture::new();
    let output = fixture.report("0", &["--json"]);
    let document: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("the report is one JSON document");
    let detail = document
        .get("inventory_incomplete")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_else(|| panic!("the report names the walk it did not run: {document}"));

    assert!(
        detail.contains("inventory not read:")
            && detail.contains("STADO_INVENTORY_BUDGET_SECONDS is 0"),
        "{detail}"
    );
    fixture.cleanup();
}

/// The `disk:` line measures the fleet's volume; `volumes[]` names every
/// device-backed filesystem beside it, the fleet's among them, and
/// `block_devices.read` says whether the host could list its disks at all.
/// On 2026-09-18 a 16 TiB disk sat attached and unmounted on the Linux
/// builder while the report said the host had 29 GiB.
#[test]
fn the_report_lists_every_volume_and_says_whether_disks_were_listed() {
    let fixture = Fixture::new();
    let output = fixture.report("0", &["--json"]);
    let document: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("the report is one JSON document");
    let fleet_volume = document["usage"]["filesystem"]
        .as_str()
        .unwrap_or_else(|| panic!("the report names the fleet's volume: {document}"));
    let volumes = document["volumes"]
        .as_array()
        .unwrap_or_else(|| panic!("the report carries volumes[]: {document}"));
    assert!(
        volumes
            .iter()
            .any(|volume| volume["filesystem"].as_str() == Some(fleet_volume)),
        "the fleet's volume {fleet_volume} is among the volumes: {volumes:?}"
    );
    assert!(
        volumes.iter().all(|volume| {
            volume["filesystem"]
                .as_str()
                .is_some_and(|device| device.starts_with("/dev/"))
                && volume["mounted_on"]
                    .as_str()
                    .is_some_and(|point| point.starts_with('/'))
        }),
        "every volume is device-backed and mounted: {volumes:?}"
    );
    let listed = document["block_devices"]["read"]
        .as_bool()
        .unwrap_or_else(|| panic!("the report says whether disks were listed: {document}"));
    let has_lsblk = Path::new("/usr/bin/lsblk").exists();
    assert_eq!(
        listed, has_lsblk,
        "block_devices.read follows whether this host has lsblk: {document}"
    );
    fixture.cleanup();
}
