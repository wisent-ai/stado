//! A connection the resolver relays and the service never answers is
//! published, reported and refused by name instead of being joined.
//!
//! One isolated deployment (`held_deployment.rs`): a local store holding
//! `registry.json`, which places `marker-api` on `marker-host` at a loopback
//! address the test listens on and never answers, and the real `stado serve
//! --resolver --target marker-host` relaying the adapter declared for it. A
//! client connects to the adapter and sends a request. The resolver must say
//! the request was sent and is waiting for an answer;
//! `~/.stado/resolver-waiting.json` must list that connection in its `answer`
//! phase on the adapter's bind; once it has waited longer than the refresh
//! interval the registry declares, `stado resolver status --json` must list
//! it under `waiting_opens` and name it as a blocker with a non-zero exit;
//! and a `stado registry pull` whose store is that adapter must be refused
//! before it is sent, with the held connection's own sentence, instead of
//! standing behind it.
//!
//! The test follows the resolver's own log line by line, so every wait ends
//! on the resolver saying what it did, or on it exiting; the one wait on the
//! clock is for the declared refresh interval to pass, read back from
//! `resolver status` as `waited_seconds`, never guessed.
use serde_json::{json, Value};
use std::fs;
use std::io::Write;
use std::net::TcpStream;
use std::process::Command;

mod held_deployment;
use held_deployment::{Deployment, CONSUMER, SERVICE, TARGET};

const ANSWER_PHASE: &str = "answer";
const HELD_SENTENCE: &str = "for the first byte of an answer on an open channel";
const NOT_ASKED: &str = "Stado object API not asked:";

/// The first wait a status report lists, or the whole report when it lists
/// none.
fn first_wait(status: &Value) -> &Value {
    match status["waiting_opens"]
        .as_array()
        .and_then(|opens| opens.first())
    {
        Some(wait) => wait,
        None => panic!("the status lists no waiting opens: {status}"),
    }
}

#[test]
fn a_relayed_request_the_service_never_answers_is_published_reported_and_refused() {
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
    target["hostnames"] = json!([hostname]);
    target["service_resolver"]["api_bind"] = json!(api_bind);
    target["service_resolver"]["adapters"] =
        json!([{"service": SERVICE, "consumer": CONSUMER, "bind": adapter_bind}]);
    let refresh_seconds = target["service_resolver"]["refresh_seconds"]
        .as_i64()
        .unwrap();
    document["service_directory"]["services"][SERVICE]["endpoints"][TARGET]["url"] =
        json!(format!("http://{service_bind}"));
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
    let said = deployment.await_line("now waiting for an answer");
    assert!(
        said.contains(&format!("request sent to {service_bind}")),
        "the resolver names the request it relayed: {said}"
    );

    let waits = deployment.published_waits();
    let [wait] = waits.as_slice() else {
        panic!("exactly one relayed request waits: {waits:?}");
    };
    assert_eq!(wait["service"], SERVICE);
    assert_eq!(wait["consumer"], CONSUMER);
    assert_eq!(wait["bind"], adapter_bind);
    assert_eq!(wait["endpoint"], service_bind);
    assert_eq!(wait["phase"], ANSWER_PHASE);

    // `resolver status` reads the age back; once it passes the declared
    // refresh interval the wait is a blocker and the verdict is not ready.
    let status = loop {
        let output = deployment.run(&["resolver", "status", "--json"]);
        let status: Value = serde_json::from_slice(&output.stdout).unwrap();
        let waited = first_wait(&status)["waited_seconds"].as_i64();
        if waited.is_some_and(|seconds| seconds > refresh_seconds) {
            assert!(
                !output.status.success(),
                "a held adapter is not a ready resolver: {status}"
            );
            break status;
        }
    };
    assert_eq!(first_wait(&status)["phase"], ANSWER_PHASE);
    assert_eq!(first_wait(&status)["bind"], adapter_bind);
    let blockers: Vec<&str> = status["blockers"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(Value::as_str)
        .collect();
    assert!(
        blockers
            .iter()
            .any(|blocker| blocker.contains(HELD_SENTENCE)
                && blocker.contains(&service_bind)
                && blocker.contains(&adapter_bind)),
        "the blocker names the held request, its adapter and the endpoint: {blockers:?}"
    );

    // A reader whose store is that adapter is refused before it sends, with
    // the held wait's own sentence, and returns instead of standing behind it.
    let refused = deployment.through_adapter(&["registry", "pull", "--generation-only"]);
    let stderr = String::from_utf8_lossy(&refused.stderr);
    assert!(
        !refused.status.success(),
        "a request through a held adapter is refused: {stderr}"
    );
    assert!(
        stderr.contains(NOT_ASKED)
            && stderr.contains(HELD_SENTENCE)
            && stderr.contains(&adapter_bind),
        "the refusal names the request not sent and the held connection it would stand behind: {stderr}"
    );

    deployment.report["outcome"] = json!("passed");
    deployment.save();
}
