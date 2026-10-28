//! A live schedule pinned to a consumer no registry target names is reported
//! by `stado registry doctor` as `schedule-pinned-nowhere`, with the command
//! that repins it; one pinned to a declared target, a removed one and one
//! with no pin are not.
//!
//! One isolated deployment: `config init` seeds a local registry naming this
//! machine; the test writes four schedules into the deployment's store (one
//! pinned to a host no target names, one pinned to the seeded target, one
//! removed while pinned nowhere, one with no pin) and reads the real `stado
//! registry doctor --json`.
// This journey reads the deployment's store and CLI only; the beacon serving
// the host-health journeys drive through it is unused here.
#[path = "../host_health/beacon/deployment.rs"]
#[allow(dead_code)]
mod deployment;

use deployment::Deployment;
use serde_json::{json, Value};
use std::fs;

const NOWHERE: &str = "macos-no-such-machine";

/// Write `schedule` as the store's `schedules/<id>.json`.
fn write_schedule(deployment: &Deployment, schedule: &Value) {
    let directory = deployment.store().join("schedules");
    fs::create_dir_all(&directory).unwrap();
    let id = schedule["schedule_id"]
        .as_str()
        .expect("a schedule names its id");
    fs::write(
        directory.join(format!("{id}.json")),
        serde_json::to_vec_pretty(schedule).unwrap(),
    )
    .unwrap();
}

/// Every finding of `kind` `stado registry doctor --json` reports, as
/// (subject, detail).
fn findings(deployment: &mut Deployment, kind: &str) -> Vec<(String, String)> {
    let output = deployment.run(&["registry", "doctor", "--json"]);
    let report: Value = match serde_json::from_slice(&output.stdout) {
        Ok(report) => report,
        Err(error) => panic!("registry doctor printed no JSON report: {error}"),
    };
    report["findings"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|finding| finding["finding"] == kind)
        .map(|finding| {
            let text = |key: &str| finding[key].as_str().into_iter().collect::<String>();
            (text("subject"), text("detail"))
        })
        .collect()
}

#[test]
fn a_schedule_pinned_to_no_declared_host_is_named() {
    let mut deployment = Deployment::start();
    let registry: Value =
        serde_json::from_slice(&fs::read(deployment.store().join("registry.json")).unwrap())
            .unwrap();
    let seeded = registry["targets"]
        .as_array()
        .and_then(|targets| targets.first())
        .and_then(|target| target["name"].as_str())
        .expect("config init seeds a registry naming this machine")
        .to_string();
    let cron = "@daily";
    write_schedule(
        &deployment,
        &json!({"schedule_id": "pinned-nowhere", "cron": cron, "command": "true", "pinned_host": NOWHERE}),
    );
    write_schedule(
        &deployment,
        &json!({"schedule_id": "pinned-here", "cron": cron, "command": "true", "pinned_host": seeded}),
    );
    write_schedule(
        &deployment,
        &json!({"schedule_id": "removed", "cron": cron, "command": "true", "pinned_host": NOWHERE, "deleted": true}),
    );
    write_schedule(
        &deployment,
        &json!({"schedule_id": "unpinned", "cron": cron, "command": "true"}),
    );

    let nowhere = findings(&mut deployment, "schedule-pinned-nowhere");
    deployment.report["schedule_pinned_nowhere"] = json!(nowhere);
    let subjects: Vec<&str> = nowhere
        .iter()
        .map(|(subject, _)| subject.as_str())
        .collect();
    assert_eq!(subjects, ["pinned-nowhere"], "{nowhere:?}");
    assert!(
        nowhere.iter().all(|(_, detail)| detail.contains(NOWHERE)
            && detail.contains("stado schedule create --id pinned-nowhere")
            && detail.contains("--repin-schedules")),
        "{nowhere:?}"
    );
    assert!(findings(&mut deployment, "unreadable-schedules").is_empty());
    deployment.pass();
}
