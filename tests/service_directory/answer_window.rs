//! A route that declares `answer_seconds` is not held by a service that never
//! begins answering: the resolver cuts the relayed request at that window,
//! announces the wait through `stado::wait` on its own stderr, and refuses
//! the client with the cause as an HTTP 502 - so a client that reads no
//! waiting list learns in seconds what it would otherwise learn never.
//!
//! One isolated deployment (`held_deployment.rs`): a local store holding
//! `registry.json`, whose `marker-api` route is placed at a loopback address
//! the test listens on and never answers, with `answer_seconds` declared on
//! the route, and the real `stado serve --resolver --target marker-host`
//! relaying the adapter declared for it. A client connects, sends a request,
//! and reads its answer to the end; a registry whose window is not positive
//! is refused by `registry import`.
use serde_json::{json, Value};
use std::fs;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::process::Command;

mod held_deployment;
use held_deployment::{Deployment, CONSUMER, SERVICE, TARGET};

/// The declared window, taken from the fixture registry's own refresh
/// interval so the test states no number of its own: a service that has not
/// begun answering within one refresh of the directory is cut.
fn window_from(document: &Value) -> u64 {
    document["targets"]
        .as_array()
        .and_then(|targets| targets.first())
        .and_then(|target| target["service_resolver"]["refresh_seconds"].as_u64())
        .expect("the fixture declares refresh_seconds")
}

#[test]
fn a_relayed_request_unanswered_within_the_declared_window_is_cut_and_the_client_told_why() {
    let mut deployment = Deployment::start();
    deployment.cli(&["config", "init"]);
    let hostname = Command::new("hostname").output().unwrap();
    let hostname = String::from_utf8(hostname.stdout)
        .unwrap()
        .trim()
        .to_lowercase();
    let (service_bind, _holder) = deployment.silent_service();
    let adapter_bind = deployment.free_bind();
    let api_bind = deployment.free_bind();
    let mut document: Value = serde_json::from_str(include_str!("registry.json")).unwrap();
    let window = window_from(&document);
    let target = document["targets"]
        .as_array_mut()
        .and_then(|targets| targets.first_mut())
        .unwrap();
    target["hostnames"] = json!([hostname]);
    target["service_resolver"]["api_bind"] = json!(api_bind);
    target["service_resolver"]["adapters"] =
        json!([{"service": SERVICE, "consumer": CONSUMER, "bind": adapter_bind}]);
    document["service_directory"]["services"][SERVICE]["endpoints"][TARGET]["url"] =
        json!(format!("http://{service_bind}"));

    // A window that is not positive is an author error the import names.
    let mut refused_document = document.clone();
    refused_document["service_directory"]["services"][SERVICE]["answer_seconds"] =
        json!(window - window);
    let refused_registry = deployment.root.join("registry-zero-window.json");
    fs::write(
        &refused_registry,
        serde_json::to_vec_pretty(&refused_document).unwrap(),
    )
    .unwrap();
    let refused = deployment.run(&["registry", "import", &refused_registry.to_string_lossy()]);
    let refusal = String::from_utf8_lossy(&refused.stderr);
    assert!(
        !refused.status.success() && refusal.contains("answer_seconds: must be positive"),
        "a zero window is refused by name: {refusal}"
    );
    deployment.report["zero_window_refusal"] = json!(refusal.trim());

    document["service_directory"]["services"][SERVICE]["answer_seconds"] = json!(window);
    let registry = deployment.root.join("registry.json");
    fs::write(&registry, serde_json::to_vec_pretty(&document).unwrap()).unwrap();
    let registry = registry.to_string_lossy().into_owned();
    deployment.cli(&["registry", "import", &registry]);

    deployment.serve();
    deployment.await_line(&format!("stado resolver target={TARGET}"));

    // The client's request goes up the relayed channel; the service holds it.
    let mut client = TcpStream::connect(&adapter_bind).unwrap();
    client
        .write_all(b"GET /never-answered HTTP/1.1\r\nHost: marker-api\r\n\r\n")
        .unwrap();
    client.flush().unwrap();
    deployment.await_line("now waiting for an answer");

    // The resolver announces the bounded wait as every Stado wait is
    // announced, then cuts the request at the window and says so.
    let announced = deployment.await_line("czekam: the first byte of an answer from");
    assert!(
        announced.contains(&format!("within the {window}s window the route declares"))
            && announced.contains(&service_bind),
        "the wait names the window and the endpoint: {announced}"
    );
    let ended = deployment.await_line("blad czekania: the first byte of an answer from");
    assert!(
        ended.contains("przyczyna:") && ended.contains("sent no byte of an answer within"),
        "the wait ends with the cause: {ended}"
    );
    // The cut request gives its place in the published waiting list back
    // before the client is answered.
    let cut = deployment.await_line("cut at the route's answer window");
    assert!(cut.contains("still waiting"), "{cut}");
    let refused = deployment.await_line("refused connection:");
    assert!(
        refused.contains("sent no byte of an answer within")
            && refused.contains(&format!("{window}s window"))
            && refused.contains(&service_bind),
        "the refusal names the endpoint and the window: {refused}"
    );

    // The client reads the cause to the end: the connection was closed, not
    // held.
    let mut answer = String::new();
    client.read_to_string(&mut answer).unwrap();
    assert!(
        answer.starts_with("HTTP/1.1 502") && answer.contains("sent no byte of an answer within"),
        "the client is told why, as HTTP, and the connection ends: {answer}"
    );
    deployment.report["client_answer"] = json!(answer.trim());
    assert!(
        deployment.published_waits().is_empty(),
        "a cut request is no longer published as waiting"
    );

    deployment.report["outcome"] = json!("passed");
    deployment.save();
}
