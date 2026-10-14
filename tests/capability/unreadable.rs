//! A host's capability measurement that cannot be read is reported as
//! unreadable, with its cause, rather than as a host nobody measured or a
//! host measured unable.
//!
//! One isolated deployment: `config init` seeds a local registry naming this
//! machine, the test publishes that host's `host_capabilities/<host>.json`
//! object into the local store in two broken shapes, and the real
//! `stado overview` reads it back, as JSON and as text.
#[path = "../host_health/beacon/deployment.rs"]
mod deployment;

use deployment::Deployment;
use serde_json::{json, Value};
use std::fs;

const SCHEMA: &str = "wisent.host-capabilities.v1";

/// The declared host the seeded registry names first.
fn declared_host(deployment: &Deployment) -> String {
    let registry: Value =
        serde_json::from_slice(&fs::read(deployment.store().join("registry.json")).unwrap())
            .unwrap();
    registry["targets"]
        .as_array()
        .and_then(|targets| targets.first())
        .and_then(|target| target["name"].as_str())
        .expect("config init seeds a registry naming this machine")
        .to_string()
}

/// Publish `body` as `host`'s capability measurement.
fn publish(deployment: &Deployment, host: &str, body: &[u8]) {
    let directory = deployment.store().join("host_capabilities");
    fs::create_dir_all(&directory).unwrap();
    fs::write(directory.join(format!("{host}.json")), body).unwrap();
}

/// The measurement `stado overview --json` reports for `host`.
fn reported_measurement(deployment: &mut Deployment, host: &str) -> Value {
    let overview: Value = serde_json::from_str(&deployment.cli(&["overview", "--json"])).unwrap();
    overview["fleet"]["targets"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|target| target["name"] == host)
        .map(|target| target["measurement"].clone())
        .expect("stado overview lists every registry target")
}

#[test]
fn an_unreadable_measurement_is_reported_with_its_cause() {
    let mut deployment = Deployment::start();
    let host = declared_host(&deployment);

    // An object that is not JSON is not a host nobody measured.
    publish(&deployment, &host, b"{ this is not json");
    let measurement = reported_measurement(&mut deployment, &host);
    deployment.report["not_json"] = measurement.clone();
    let reason = measurement["unreadable"]
        .as_str()
        .expect("a measurement that is not JSON is reported as unreadable");
    assert!(reason.contains("is not JSON"), "{measurement}");
    assert!(reason.contains(&format!("{host}.json")), "{measurement}");

    // A capability without a true or false value is not a capability
    // measured false.
    let garbled = json!({
        "schema": SCHEMA,
        "measured_at": chrono::Utc::now().to_rfc3339(),
        "capabilities": {"browser-login": {"detail": "probe wrote no answer"}},
    });
    publish(&deployment, &host, &serde_json::to_vec(&garbled).unwrap());
    let measurement = reported_measurement(&mut deployment, &host);
    deployment.report["no_value"] = measurement.clone();
    let reason = measurement["unreadable"]
        .as_str()
        .expect("a capability without a value makes the measurement unreadable");
    assert!(
        reason.contains("browser-login") && reason.contains("without a true or false value"),
        "{measurement}"
    );

    // The text the operator reads says the same.
    let text = deployment.cli(&["overview"]);
    assert!(
        text.contains(&format!("{host} its measurement cannot be read")),
        "{text}"
    );

    // A readable measurement carries no such cause and keeps its answers.
    let readable = json!({
        "schema": SCHEMA,
        "measured_at": chrono::Utc::now().to_rfc3339(),
        "capabilities": {"browser-login": {"value": false}},
    });
    publish(&deployment, &host, &serde_json::to_vec(&readable).unwrap());
    let measurement = reported_measurement(&mut deployment, &host);
    deployment.report["readable"] = measurement.clone();
    assert!(measurement["unreadable"].is_null(), "{measurement}");
    assert_eq!(
        measurement["capabilities"]["browser-login"],
        json!({"value": false, "detail": null}),
        "{measurement}"
    );
    deployment.pass();
}
