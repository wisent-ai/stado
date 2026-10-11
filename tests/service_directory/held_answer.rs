//! A resolver never cuts a relayed request by a clock: a request the service
//! never answers stays published as waiting (`stado resolver status`), where
//! every caller that reads the list before sending is refused with the held
//! wait's sentence. A route that still declares the retired `answer_seconds`
//! is refused by `registry import`, naming the key.
//!
//! One isolated deployment (`held_deployment.rs`): a local store holding
//! `registry.json`, whose `marker-api` route is placed at a loopback address
//! the test listens on and never answers, and the real `stado serve
//! --resolver --target marker-host` relaying the adapter declared for it. A
//! client connects and sends a request; the request is published as waiting
//! and the connection is not cut.
use serde_json::{json, Value};
use std::fs;
use std::io::Write;
use std::net::TcpStream;
use std::process::Command;

mod held_deployment;
use held_deployment::{Deployment, CONSUMER, SERVICE, TARGET};

#[test]
fn a_held_request_stays_published_and_the_retired_answer_window_is_refused() {
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
    let target = document["targets"]
        .as_array_mut()
        .and_then(|targets| targets.first_mut())
        .unwrap();
    // Any value the retired key carries is refused; the fixture's own
    // refresh interval is the one a former route would have declared.
    let retired_window = target["service_resolver"]["refresh_seconds"].clone();
    target["hostnames"] = json!([hostname]);
    target["service_resolver"]["api_bind"] = json!(api_bind);
    target["service_resolver"]["adapters"] =
        json!([{"service": SERVICE, "consumer": CONSUMER, "bind": adapter_bind}]);
    document["service_directory"]["services"][SERVICE]["endpoints"][TARGET]["url"] =
        json!(format!("http://{service_bind}"));

    // A route that still declares the retired window is refused, naming it.
    let mut retired_document = document.clone();
    retired_document["service_directory"]["services"][SERVICE]["answer_seconds"] = retired_window;
    let retired_registry = deployment.root.join("registry-retired-window.json");
    fs::write(
        &retired_registry,
        serde_json::to_vec_pretty(&retired_document).unwrap(),
    )
    .unwrap();
    let refused = deployment.run(&["registry", "import", &retired_registry.to_string_lossy()]);
    let refusal = String::from_utf8_lossy(&refused.stderr);
    assert!(
        !refused.status.success() && refusal.contains("answer_seconds"),
        "a route declaring the retired window is refused by name: {refusal}"
    );
    deployment.report["retired_window_refusal"] = json!(refusal.trim());

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
    let waiting = deployment.await_line("now waiting for an answer");
    assert!(waiting.contains(&service_bind), "{waiting}");

    // The held request is published, not cut: it stays in the list a caller
    // reads before sending.
    let published = deployment.published_waits();
    assert!(
        !published.is_empty(),
        "a held request is published as waiting"
    );
    deployment.report["published_waits"] = json!(published);

    deployment.report["outcome"] = json!("passed");
    deployment.save();
}
