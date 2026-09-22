//! Actual host-service sockets and failure propagation, using the existing
//! isolated local registry. No credentials or state from the operator are used.

use std::io::Read;
use std::process::Command;

use serde_json::json;

use crate::fixture::{http_get, listening, wait_listening, wait_until, Policy, Serving};
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

#[test]
fn finite_proxy_commands_share_the_host_pid_and_stop_only_their_listener() {
    let policy = Policy::patient(FIRST_GENERATION);
    let host = host(&policy);
    let arguments = arguments(&policy);
    let args: Vec<&str> = arguments.iter().map(String::as_str).collect();
    let mut service = Serving::start(&host, &args);
    assert!(wait_listening(policy.upstream), "{}", service.said());
    let (reservation, port) = held_port();
    let bind = format!("127.0.0.1:{port}");
    let state = host.root.path().join("proxy.json");
    std::fs::write(
        &state,
        serde_json::to_vec(&json!({
            "generation": 1,
            "upstream": format!("127.0.0.1:{}", policy.upstream),
            "updated_at": "2026-09-22T00:00:00Z"
        }))
        .unwrap(),
    )
    .expect("real forwarding configuration");
    let state = state.to_str().expect("state path");
    let client = host.root.path().join("copied-stado");
    std::fs::copy(env!("CARGO_BIN_EXE_stado"), &client)
        .expect("separately installed native client");
    drop(reservation);
    let ensure = host
        .command_at(
            &client,
            &["release", "proxy", "--state", state, "--bind", &bind],
        )
        .output()
        .expect("finite proxy command");
    assert!(ensure.status.success(), "{}", said(&ensure));
    let response = http_get(port, "/healthz", &[]).expect("forwarded actual API");
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    let owned = Command::new("lsof")
        .args([
            "-nP",
            "-a",
            "-p",
            &service.pid().to_string(),
            &format!("-iTCP:{port}"),
            "-sTCP:LISTEN",
            "-t",
        ])
        .output()
        .expect("native proxy ownership");
    assert!(owned.status.success(), "{}", said(&owned));
    assert_eq!(
        String::from_utf8_lossy(&owned.stdout).trim(),
        service.pid().to_string()
    );
    let mut connection = std::net::TcpStream::connect(&bind).expect("open forwarding connection");
    connection
        .set_nonblocking(true)
        .expect("nonblocking observation");
    let stopped = host.stado(&[
        "release", "proxy", "--state", state, "--bind", &bind, "--stop",
    ]);
    assert!(stopped.status.success(), "{}", said(&stopped));
    assert!(!listening(port), "removed proxy still listens");
    assert!(
        wait_until(|| match connection.read(&mut [0; 1]) {
            Ok(0) => true,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => false,
            other => panic!("unexpected stopped connection result: {other:?}"),
        }),
        "removed proxy retained a connection"
    );
    let response =
        http_get(policy.upstream, "/healthz", &[]).expect("host API after proxy removal");
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    assert!(service.running(), "{}", service.said());
}

#[test]
fn another_listener_is_refused_without_stopping_the_host() {
    let policy = Policy::patient(FIRST_GENERATION);
    let host = host(&policy);
    let arguments = arguments(&policy);
    let args: Vec<&str> = arguments.iter().map(String::as_str).collect();
    let mut service = Serving::start(&host, &args);
    assert!(wait_listening(policy.upstream), "{}", service.said());
    let (occupied, port) = held_port();
    let bind = format!("127.0.0.1:{port}");
    let state = host.root.path().join("occupied-proxy.json");
    std::fs::write(
        &state,
        serde_json::to_vec(&json!({
            "generation": 1,
            "upstream": format!("127.0.0.1:{}", policy.upstream),
            "updated_at": "2026-09-22T00:00:00Z"
        }))
        .unwrap(),
    )
    .expect("real forwarding configuration");
    let refused = host.stado(&[
        "release",
        "proxy",
        "--state",
        state.to_str().unwrap(),
        "--bind",
        &bind,
    ]);
    assert!(!refused.status.success(), "{}", said(&refused));
    assert!(said(&refused).contains(&bind), "{}", said(&refused));
    let _client = std::net::TcpStream::connect(&bind).expect("original listener still accepts");
    let _accepted = occupied
        .accept()
        .expect("the unrelated owner retained its listener");
    assert!(service.running(), "{}", service.said());
    let response = http_get(policy.upstream, "/healthz", &[]).expect("API after refused proxy");
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
}
