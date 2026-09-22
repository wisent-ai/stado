//! Actual host-service sockets and failure propagation, using the existing
//! isolated local registry. No credentials or state from the operator are used.

use std::process::Command;

use serde_json::json;

use crate::fixture::{http_get, listening, wait_listening, Policy, Serving};
use crate::{held_port, said, Host, TARGET};

const COORDINATOR: &str = "runtime-coordinator";
// The first published service-directory generation in this isolated registry.
const FIRST_GENERATION: u64 = 1;

fn host(policy: &Policy) -> Host {
    let mut document = policy.document();
    document["coordinators"] = json!([{
        "name": COORDINATOR,
        "runtime": "daemon",
        "active": true,
        "interval_seconds": policy.refresh_seconds,
    }]);
    Host::new(&document)
}

fn arguments(policy: &Policy) -> Vec<String> {
    vec![
        "serve".into(),
        "--target".into(),
        TARGET.into(),
        "--coordinator".into(),
        COORDINATOR.into(),
        "--bind".into(),
        "127.0.0.1".into(),
        "--port".into(),
        policy.upstream.to_string(),
        "--release-interval-seconds".into(),
        policy.refresh_seconds.to_string(),
        "--health-interval-seconds".into(),
        policy.refresh_seconds.to_string(),
    ]
}

#[test]
fn api_and_resolver_are_served_by_the_same_host_process() {
    let policy = Policy::patient(FIRST_GENERATION);
    let host = host(&policy);
    let arguments = arguments(&policy);
    let args: Vec<&str> = arguments.iter().map(String::as_str).collect();
    let mut service = Serving::start(&host, &args);
    for port in [policy.api, policy.adapter, policy.upstream] {
        assert!(wait_listening(port), "port {port}: {}", service.said());
    }
    let response = http_get(policy.adapter, "/healthz", &[]).expect("real adapter response");
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    let (_, body) = response.split_once("\r\n\r\n").expect("HTTP response body");
    let health: serde_json::Value = serde_json::from_str(body).expect("actual API health response");
    // No Skarbiec grants are installed in this fixture. Sharing a process must
    // not turn that missing authorization into a healthy protected boundary.
    assert_eq!(health["degraded"], true, "{health}");

    let sockets = Command::new("lsof")
        .args([
            "-nP",
            "-a",
            "-p",
            &service.pid().to_string(),
            "-iTCP",
            "-sTCP:LISTEN",
            "-Fn",
        ])
        .output()
        .expect("blocked: the operating system cannot inspect socket ownership");
    assert!(sockets.status.success(), "{}", said(&sockets));
    let owned = String::from_utf8(sockets.stdout).expect("socket ownership is UTF-8");
    for port in [policy.api, policy.adapter, policy.upstream] {
        assert!(
            owned
                .lines()
                .any(|line| line == format!("n127.0.0.1:{port}")),
            "{owned}"
        );
    }
    assert!(service.running(), "{}", service.said());
}

#[test]
fn a_failed_resolver_ends_the_host_service_without_leaving_its_api() {
    let mut policy = Policy::patient(FIRST_GENERATION);
    let (_occupied, port) = held_port();
    policy.api = port;
    let host = host(&policy);
    let arguments = arguments(&policy);
    let args: Vec<&str> = arguments.iter().map(String::as_str).collect();
    let output = host.stado(&args);
    assert!(!output.status.success(), "{}", said(&output));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("component=resolver"),
        "{}",
        said(&output)
    );
    assert!(
        !listening(policy.upstream),
        "the failed service left its API listening"
    );
    assert!(
        !listening(policy.adapter),
        "the failed service left its adapter listening"
    );
}
