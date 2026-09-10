//! The memory half of `runner diagnostics`, confronted with this machine.
//!
//! A runner that will not start because `Failed to create CoreCLR, HRESULT:
//! 0x8007000C` needs three facts beside its log: what the host had, what the
//! unit was allowed, and whether anything reclaims memory on that host. Every
//! one of them is a reading, so every one of them is checked here against an
//! independent answer from the same machine rather than against itself.

use std::process::Command;

use crate::fixture::{report, stderr, Fixture, TARGET};

/// This machine's total memory in MiB, read without the product.
fn total_mebibytes() -> i64 {
    let mebibyte = 1024 * 1024;
    if std::env::consts::OS == "macos" {
        let output = Command::new("/usr/sbin/sysctl")
            .args(["-n", "hw.memsize"])
            .output()
            .expect("sysctl reports this machine's memory");
        String::from_utf8_lossy(&output.stdout)
            .trim()
            .parse::<i64>()
            .expect("hw.memsize is a number")
            / mebibyte
    } else {
        let meminfo = std::fs::read_to_string("/proc/meminfo").expect("this kernel reports memory");
        meminfo
            .lines()
            .find_map(|line| line.strip_prefix("MemTotal:"))
            .and_then(|value| value.split_whitespace().next())
            .and_then(|kib| kib.parse::<i64>().ok())
            .expect("MemTotal is a number")
            / 1024
    }
}

#[test]
fn diagnostics_report_this_machine_memory_beside_the_runner_log() {
    let fixture = Fixture::new();
    let profiles = fixture.declared_profiles();
    let name = profiles[0]["name"].as_str().expect("a profile name");

    let output = fixture.diagnostics(name);
    assert!(output.status.success(), "{}", stderr(&output));
    let report = report(&output);
    let memory = &report["memory"];

    let total = memory["total_mb"].as_i64();
    assert_eq!(
        total,
        Some(total_mebibytes()),
        "the diagnostics must report this machine's own total memory: {memory}"
    );
    let available = memory["available_mb"]
        .as_i64()
        .expect("the diagnostics report available memory");
    assert!(
        available > 0 && available <= total.unwrap(),
        "available memory must be a real reading bounded by the total: {memory}"
    );
    assert!(
        memory["swap"].as_str().is_some_and(|swap| !swap.is_empty()),
        "the swap reading is part of the diagnosis: {memory}"
    );
    assert!(
        memory["detail"]
            .as_str()
            .is_some_and(|detail| detail.contains("memory")),
        "the diagnosis must say what the readings mean: {memory}"
    );
}

#[test]
fn diagnostics_report_the_reclaim_verdict_the_watermark_command_reports() {
    let fixture = Fixture::new();
    let profiles = fixture.declared_profiles();
    let name = profiles[0]["name"].as_str().expect("a profile name");

    let watermark = fixture.stado(&["space", "watermark", TARGET, "--json"]);
    assert!(watermark.status.success(), "{}", stderr(&watermark));
    let declared = report(&watermark);

    let diagnostics = fixture.diagnostics(name);
    assert!(diagnostics.status.success(), "{}", stderr(&diagnostics));
    let reclaim = report(&diagnostics)["memory"]["reclaim"].clone();

    assert_eq!(
        reclaim, declared["automatic"],
        "a runner diagnosis and the watermark command must not disagree about whether this \
         host repairs its memory"
    );
}

#[test]
fn diagnostics_say_when_the_runner_last_wrote_a_log() {
    let fixture = Fixture::new();
    let profiles = fixture.declared_profiles();
    let name = profiles[0]["name"].as_str().expect("a profile name");

    let report = report(&fixture.diagnostics(name));
    let host_time = report["host_time"]
        .as_str()
        .expect("the diagnostics stamp the host clock");
    assert!(
        host_time.ends_with('Z') && host_time.len() == "2026-09-10T10:41:24Z".len(),
        "the host clock must be reported in UTC: {host_time}"
    );
    for key in ["log_modified_at", "wrapper_log_modified_at"] {
        let value = report[key].as_str().unwrap_or_default();
        assert!(
            value == "unknown" || value.ends_with('Z'),
            "{key} must be a UTC stamp or an honest 'unknown': {value}"
        );
    }
}
