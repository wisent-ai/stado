//! `stado fleet needs`, driven as the real binary against an isolated
//! store: what the fleet lacks, from what it publishes and refuses.
//!
//! The advisor never invents a demand: an empty store yields the documented
//! empty sentence; a host publishing memory pressure and a disk shortfall
//! yields one `ram` and one `storage` need naming it with its numbers; a
//! placement no host in the fleet can take — here `gui-automation`, which
//! needs a Mac, on a fleet that declares only Linux — is recorded by the
//! refusing product path and comes back as a `host` need for that platform.

mod support;

use serde_json::Value;

use support::{Journey, HOST};

/// The readings the fixture publishes, as the advisor prints them.
const PRESSED_AVAILABLE: &str = "1.5";
const PRESSED_SWAP: &str = "90.0";
const PRESSED_FREE_DISK: &str = "6.0";
const DECLARED_DISK_TARGET: &str = "20";
/// Twice the seeded 16 GiB, because swap is over its watermark.
const SUGGESTED_RAM: &str = "32";
const DEFAULT_WINDOW: &str = "7";

fn report(journey: &Journey) -> Value {
    let output = journey.invoke(&["fleet", "needs", "--json"]);
    assert!(
        output.status.success(),
        "fleet needs failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "fleet needs printed no JSON report: {error}\n{}",
            String::from_utf8_lossy(&output.stdout)
        )
    })
}

fn needs_of<'a>(report: &'a Value, kind: &str) -> Vec<&'a Value> {
    report["needs"]
        .as_array()
        .unwrap_or_else(|| panic!("the report lists needs: {report}"))
        .iter()
        .filter(|need| need["need"] == kind)
        .collect()
}

#[test]
fn an_idle_fleet_reports_no_need_in_the_documented_sentence() {
    let journey = Journey::new();
    let text = journey.invoke(&["fleet", "needs"]);
    assert!(text.status.success());
    assert_eq!(
        String::from_utf8_lossy(&text.stdout).trim(),
        format!("the fleet reports no unmet need in the last {DEFAULT_WINDOW} days")
    );
    let document = report(&journey);
    assert_eq!(document["needs"], serde_json::json!([]), "{document}");
    assert_eq!(
        document["window_days"].to_string(),
        DEFAULT_WINDOW,
        "{document}"
    );
}

#[test]
fn a_host_over_its_watermarks_yields_ram_and_storage_needs_with_its_numbers() {
    let journey = Journey::new();
    journey.publish_pressed_host();
    let document = report(&journey);

    let ram = needs_of(&document, "ram");
    assert_eq!(ram.len(), 1, "{document}");
    assert_eq!(ram[0]["target"], HOST, "{document}");
    assert_eq!(ram[0]["severity"], "high", "{document}");
    assert_eq!(
        ram[0]["summary"],
        format!(
            "{HOST} is short of memory: {PRESSED_AVAILABLE} GiB available and swap at {PRESSED_SWAP}%"
        ),
        "{document}"
    );
    assert_eq!(
        ram[0]["suggestion"],
        format!("add memory to {HOST} or replace it with a machine of about {SUGGESTED_RAM} GiB"),
        "{document}"
    );

    let storage = needs_of(&document, "storage");
    assert_eq!(storage.len(), 1, "{document}");
    assert_eq!(storage[0]["target"], HOST, "{document}");
    assert_eq!(storage[0]["severity"], "high", "{document}");
    assert_eq!(
        storage[0]["summary"],
        format!(
            "{HOST} has {PRESSED_FREE_DISK} GiB free against a declared target of {DECLARED_DISK_TARGET} GiB"
        ),
        "{document}"
    );
    assert!(
        needs_of(&document, "host").is_empty(),
        "a platform need was invented without demand: {document}"
    );

    let text = String::from_utf8_lossy(&journey.invoke(&["fleet", "needs"]).stdout).into_owned();
    assert!(
        text.starts_with(&format!("high ram ({HOST}): {HOST} is short of memory")),
        "{text}"
    );
}

#[test]
fn a_placement_no_host_can_take_becomes_a_platform_need() {
    let journey = Journey::new();
    journey.without_this_host();
    let plan = journey.home.join("gui.plan.json");
    std::fs::write(
        &plan,
        serde_json::to_string(&serde_json::json!({
            "schema": "wisent.gui-automation-plan.v1",
            "operation": "enable"
        }))
        .unwrap(),
    )
    .unwrap();
    let refused = journey.invoke(&[
        "workload",
        "run",
        "gui-automation",
        "--plan",
        plan.to_str().unwrap(),
    ]);
    let stderr = String::from_utf8_lossy(&refused.stderr).into_owned();
    assert!(
        !refused.status.success(),
        "a Linux-only fleet ran a Mac workload"
    );
    assert!(
        stderr.contains(
            "the fleet declares no gui-automation; add it to stado-rs/data/work/workloads.json"
        ),
        "{stderr}"
    );

    let document = report(&journey);
    let hosts = needs_of(&document, "host");
    assert_eq!(hosts.len(), 1, "{document}");
    assert_eq!(hosts[0]["platform"], "darwin-arm64", "{document}");
    assert_eq!(hosts[0]["severity"], "high", "{document}");
    assert_eq!(
        hosts[0]["summary"],
        "no declared host runs darwin-arm64; 1 placement(s) refused and 0 queued job(s) waiting for it",
        "{document}"
    );
    assert_eq!(
        hosts[0]["suggestion"],
        "add a darwin-arm64 machine to the fleet and register it with `stado registry host add`",
        "{document}"
    );
}
