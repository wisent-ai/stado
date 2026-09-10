//! Disk watermark writes through the real product binary, against the
//! isolated current-host registry.
//!
//! `stado space watermark` used to write memory only; the disk floor a
//! release build must fit above (`release_scratch_short`) could be written
//! from Stado Desktop and from nowhere on the command line.

use crate::fixture::{Host, TARGET};
use serde_json::Value;
use std::fs;

fn registry(host: &Host) -> Value {
    serde_json::from_slice(&fs::read(host.storage.join("registry.json")).unwrap()).unwrap()
}

#[test]
fn a_disk_watermark_write_lands_in_disk_cleanup_and_reads_back() {
    let host = Host::new();
    let before = registry(&host);
    let written = host.json(&[
        "space",
        "watermark",
        TARGET,
        "--disk-low-free-gb",
        "6",
        "--disk-target-free-gb",
        "9",
        "--json",
    ]);
    assert_eq!(written["target"], TARGET);
    assert_eq!(written["disk_cleanup"]["low_free_gb"], 6);
    assert_eq!(written["disk_cleanup"]["target_free_gb"], 9);
    assert!(
        written.get("memory_reclaim").is_none(),
        "a disk-only write reported a memory declaration: {written}"
    );
    let after = registry(&host);
    let policy = &after["targets"][0]["disk_cleanup"];
    assert_eq!(policy["low_free_gb"], 6);
    assert_eq!(policy["target_free_gb"], 9);
    // Everything the flags did not name is exactly what the fixture declared.
    for key in [
        "mode",
        "check_interval_seconds",
        "max_bytes_per_pass",
        "max_items_per_pass",
        "max_scan_items",
        "cleaners",
    ] {
        assert_eq!(
            policy[key], before["targets"][0]["disk_cleanup"][key],
            "{key} changed under a watermark write"
        );
    }
    let read = host.json(&["space", "watermark", TARGET, "--json"]);
    assert_eq!(read["disk_cleanup"]["low_free_gb"], 6);
    assert_eq!(read["disk_cleanup"]["target_free_gb"], 9);
    // The host's own gate reads the new floor, so the write is what the
    // scheduler and the release coordinator will measure the host against.
    let report = host.json(&["space", "report", TARGET, "--json"]);
    assert_eq!(report["free_space"]["low_watermark_bytes"], 6_u64 << 30);
    assert_eq!(report["free_space"]["target_watermark_bytes"], 9_u64 << 30);
}

#[test]
fn an_undeclared_host_is_seeded_from_the_reporting_default_before_its_floor_is_written() {
    let host = Host::new();
    host.declare("null");
    let written = host.json(&[
        "space",
        "watermark",
        TARGET,
        "--disk-low-free-gb",
        "4",
        "--disk-target-free-gb",
        "7",
        "--json",
    ]);
    let policy = &written["disk_cleanup"];
    assert_eq!(policy["low_free_gb"], 4);
    assert_eq!(policy["target_free_gb"], 7);
    assert_eq!(
        policy["mode"], "report",
        "a seeded declaration must not enforce"
    );
    assert!(policy["cleaners"].is_object());
    assert_eq!(registry(&host)["targets"][0]["disk_cleanup"], *policy);
}

#[test]
fn a_floor_at_or_above_its_target_is_refused_and_the_registry_is_unchanged() {
    let host = Host::new();
    let before = fs::read(host.storage.join("registry.json")).unwrap();
    for pair in [["12", "12"], ["12", "8"]] {
        let refused = host.run(&[
            "space",
            "watermark",
            TARGET,
            "--disk-low-free-gb",
            pair[0],
            "--disk-target-free-gb",
            pair[1],
        ]);
        assert!(!refused.status.success(), "{pair:?} was accepted");
        let stderr = String::from_utf8_lossy(&refused.stderr);
        assert!(
            stderr.contains("target_free_gb") && stderr.contains("greater than low_free_gb"),
            "the refusal does not name the rule: {stderr}"
        );
        assert_eq!(
            fs::read(host.storage.join("registry.json")).unwrap(),
            before
        );
    }
    let refused = host.run(&[
        "space",
        "watermark",
        TARGET,
        "--policy",
        "linux-queue-host",
        "--disk-low-free-gb",
        "5",
    ]);
    assert!(!refused.status.success());
    assert!(String::from_utf8_lossy(&refused.stderr)
        .contains("--policy applies a declared memory policy"));
    assert_eq!(
        fs::read(host.storage.join("registry.json")).unwrap(),
        before
    );
}

#[test]
fn disk_and_memory_fields_in_one_call_land_in_one_generation() {
    let host = Host::new();
    let written = host.json(&[
        "space",
        "watermark",
        TARGET,
        "--disk-low-free-gb",
        "5",
        "--disk-target-free-gb",
        "8",
        "--memory-low-free-mb",
        "1024",
        "--memory-target-free-mb",
        "2048",
        "--json",
    ]);
    assert_eq!(written["disk_cleanup"]["low_free_gb"], 5);
    assert_eq!(written["memory_reclaim"]["low_free_mb"], 1024);
    assert_eq!(written["memory_reclaim"]["target_free_mb"], 2048);
    let generation = written["generation"]
        .as_str()
        .expect("one write reports one generation")
        .to_owned();
    let after = registry(&host);
    assert_eq!(after["targets"][0]["disk_cleanup"]["target_free_gb"], 8);
    assert_eq!(after["targets"][0]["memory_reclaim"]["low_free_mb"], 1024);
    // A second write moves the generation on from the one both landed in,
    // and leaves the field it did not name where the first write put it.
    let again = host.json(&[
        "space",
        "watermark",
        TARGET,
        "--disk-target-free-gb",
        "9",
        "--json",
    ]);
    assert_ne!(again["generation"].as_str(), Some(generation.as_str()));
    assert_eq!(again["disk_cleanup"]["low_free_gb"], 5);
    assert_eq!(again["disk_cleanup"]["target_free_gb"], 9);
}
