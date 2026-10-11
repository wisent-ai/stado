//! The host process's scans of its own disk are incremental, admitted by the
//! host's load ceiling, and published when skipped.
//!
//! One isolated deployment runs the real `stado serve` with the health role
//! on a one-second cadence. The first beacon reads launchd's event log for
//! every declared unit; the second carries those reads, because every unit
//! is as it was, and publishes the load reading the carry was decided on
//! against the measured core ceiling. Then `host.scan_load_ceiling` is
//! declared as the lowest load a host can be above, and the next beacon
//! carries every unit because the host is above its ceiling, names the
//! skipped scan under `scans.skipped`, and `stado host health` shows it.
mod deployment;

use deployment::Deployment;
use serde_json::Value;
use std::fs;

const UNIT_EVENTS: &str = "unit_events";
const CARRIED_SAME: &str = "carried: the unit is as it was at the previous read";
const CARRIED_LOAD: &str = "carried: the host is above its scan load ceiling";

fn beacon(deployment: &Deployment, slug: &str) -> Value {
    let path = deployment
        .store()
        .join("host_health")
        .join(format!("{slug}.json"));
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) => panic!(
            "the stored beacon {} must be readable: {error}",
            path.display()
        ),
    };
    serde_json::from_slice(&bytes).expect("the stored beacon is JSON")
}

/// The `events` field of every unit the beacon read, by unit.
fn unit_events(beacon: &Value) -> Vec<(String, String)> {
    beacon["units"]
        .as_object()
        .expect("a beacon carries its units")
        .iter()
        .filter_map(|(unit, entry)| {
            entry
                .get("events")
                .and_then(Value::as_str)
                .map(|events| (unit.clone(), events.to_string()))
        })
        .collect()
}

#[test]
fn unit_event_reads_are_carried_and_skipped_above_the_declared_ceiling() {
    let mut deployment = Deployment::start();
    let slug = deployment.serve();

    // Pass one: every unit's event log is read; no scan has asked the
    // ceiling yet, so the block carries no reading.
    let first = beacon(&deployment, &slug);
    assert!(
        first["scans"].is_object(),
        "a beacon about this machine carries its scans block: {first}"
    );
    assert!(
        first["scans"]["last"].is_null(),
        "before any scan asked, the block carries no reading: {first}"
    );
    let read = unit_events(&first);
    assert!(
        !read.is_empty(),
        "this journey needs declared units whose events the beacon reads: {first}"
    );
    for (unit, events) in &read {
        assert!(
            !events.starts_with("carried:"),
            "the first pass reads {unit}'s events itself: {events}"
        );
    }
    deployment.report["first_beacon"] = first;

    // Pass two: every unit is as it was, so its events are carried, and the
    // carry was decided on a reading against the measured core ceiling.
    let second_slug = deployment.next_publication();
    assert_eq!(second_slug, slug);
    let second = beacon(&deployment, &slug);
    for (unit, events) in unit_events(&second) {
        assert!(
            events.starts_with(CARRIED_SAME),
            "the second pass carries {unit}'s events: {events}"
        );
    }
    let last = &second["scans"]["last"];
    assert_eq!(
        last["ceiling_source"], "measured_cores",
        "nothing is declared, so the ceiling is the measured core count: {second}"
    );
    let cores = std::thread::available_parallelism().unwrap().get() as f64;
    assert_eq!(
        last["ceiling"].as_f64(),
        Some(cores),
        "the measured ceiling is this host's core count: {second}"
    );
    let Some(load) = last["load"].as_f64() else {
        panic!("the reading carries the load average: {second}");
    };
    assert!(
        second["scans"]["skipped"]
            .as_object()
            .is_some_and(|skipped| skipped.is_empty()),
        "under the ceiling no scan stands skipped: {second}"
    );
    deployment.report["second_beacon"] = second;

    // Pass three: the ceiling is declared as the lowest load average a host
    // can be above, so any load the process measures is above it and the
    // event read is skipped, carried, and published; a declaration just
    // under the measured load would be crossed again as the average moves.
    let ceiling = f64::MIN_POSITIVE;
    assert!(
        load > ceiling,
        "this journey needs a host under some load; the process measured {load}"
    );
    deployment.cli(&[
        "config",
        "set",
        "host.scan_load_ceiling",
        &ceiling.to_string(),
    ]);
    let mut third = beacon(&deployment, &slug);
    while third["scans"]["skipped"].get(UNIT_EVENTS).is_none() {
        let next = deployment.next_publication();
        assert_eq!(next, slug);
        third = beacon(&deployment, &slug);
    }
    for (unit, events) in unit_events(&third) {
        assert!(
            events.starts_with(CARRIED_LOAD),
            "above the ceiling {unit}'s events are carried without a read: {events}"
        );
    }
    let skip = &third["scans"]["skipped"][UNIT_EVENTS];
    assert_eq!(skip["ceiling_source"], "declared", "{third}");
    assert_eq!(skip["ceiling"].as_f64(), Some(ceiling), "{third}");
    assert!(
        skip["since"].is_string() && skip["latest"].is_string(),
        "{third}"
    );
    let registry: Value =
        serde_json::from_slice(&fs::read(deployment.store().join("registry.json")).unwrap())
            .unwrap();
    let mut targets = registry["targets"]
        .as_array()
        .expect("config init seeds a registry with targets")
        .iter();
    let (Some(declared), None) = (targets.next(), targets.next()) else {
        panic!("config init seeds exactly one target, this machine: {registry}");
    };
    let target = declared["name"].as_str().unwrap().to_string();
    let shown = deployment.cli(&["host", "health", &target]);
    assert!(
        shown
            .lines()
            .any(|line| line.starts_with("scans: ") && line.contains("unit_events skipped since")),
        "host health shows the skipped scan: {shown}"
    );
    deployment.report["third_beacon"] = third;
    deployment.report["host_health"] = Value::String(shown);
    deployment.pass();
}
