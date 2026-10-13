//! A connection the resolver relays and the service never answers is
//! published, reported and refused by name instead of being joined.
//!
//! One isolated deployment (`held_deployment.rs`): a local store holding
//! `registry.json`, which places `marker-api` on `marker-host` at a loopback
//! address the test listens on and never answers, and the real `stado serve
//! --resolver --target marker-host` relaying the adapter declared for it. Two
//! clients connect to the adapter and each sends a request. The resolver must
//! say each request was sent and is waiting for an answer;
//! `~/.stado/resolver-waiting.json` must list both connections in their
//! `answer` phase on the adapter's bind; once both have waited longer than
//! the refresh interval the registry declares, `stado resolver status --json`
//! must list both under `waiting_opens`, name the route once as a blocker
//! that counts them and names the oldest, and exit non-zero;
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
use std::io::{Read, Write};
use std::net::TcpStream;
use std::process::Command;

use crate::held_deployment::{Deployment, CONSUMER, SERVICE, TARGET};

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
    let mut document: Value = serde_json::from_str(include_str!("../registry.json")).unwrap();
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
    let own_address = client.local_addr().unwrap().to_string();
    assert!(
        said.contains(&format!(
            "request from {own_address} sent to {service_bind}"
        )),
        "the resolver names the request it relayed and the client it came from: {said}"
    );

    // A second client on the same adapter, before the first has waited past
    // the refresh interval, is relayed too and stands behind the same
    // service: the route now holds two connections.
    let mut later = TcpStream::connect(&adapter_bind).unwrap();
    later
        .write_all(b"GET /never-answered-either HTTP/1.1\r\nHost: marker-api\r\n\r\n")
        .unwrap();
    later.flush().unwrap();
    let later_address = later.local_addr().unwrap().to_string();
    deployment.await_line(&format!(
        "request from {later_address} sent to {service_bind}"
    ));

    let waits = deployment.published_waits();
    let [first, second] = waits.as_slice() else {
        panic!("exactly two relayed requests wait: {waits:?}");
    };
    for wait in [first, second] {
        assert_eq!(wait["service"], SERVICE);
        assert_eq!(wait["consumer"], CONSUMER);
        assert_eq!(wait["bind"], adapter_bind);
        assert_eq!(wait["endpoint"], service_bind);
        assert_eq!(wait["phase"], ANSWER_PHASE);
    }
    let clients: Vec<&Value> = vec![&first["client"], &second["client"]];
    assert!(
        clients.contains(&&json!(own_address)) && clients.contains(&&json!(later_address)),
        "each published wait names the client holding it: {waits:?}"
    );

    // `resolver status` reads the ages back; once both pass the declared
    // refresh interval the waits are a blocker and the verdict is not ready.
    let status = loop {
        let output = deployment.run(&["resolver", "status", "--json"]);
        let status: Value = serde_json::from_slice(&output.stdout).unwrap();
        let Some(opens) = status["waiting_opens"].as_array() else {
            panic!("the status carries no waiting_opens list: {status}");
        };
        let all_past = !opens.is_empty()
            && opens.iter().all(|open| {
                open["waited_seconds"]
                    .as_i64()
                    .is_some_and(|seconds| seconds > refresh_seconds)
            });
        if all_past {
            assert!(
                !output.status.success(),
                "a held adapter is not a ready resolver: {status}"
            );
            break status;
        }
    };
    assert_eq!(first_wait(&status)["phase"], ANSWER_PHASE);
    assert_eq!(first_wait(&status)["bind"], adapter_bind);
    assert_eq!(
        status["waiting_opens"].as_array().map(Vec::len),
        Some(waits.len()),
        "the JSON list keeps every held connection: {status}"
    );
    let blockers: Vec<&str> = status["blockers"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(Value::as_str)
        .collect();
    let held: Vec<&&str> = blockers
        .iter()
        .filter(|blocker| blocker.contains(HELD_SENTENCE))
        .collect();
    let [route] = held.as_slice() else {
        panic!(
            "two connections on one route are one blocker, not one per connection: {blockers:?}"
        );
    };
    let own_port = client.local_addr().unwrap().port().to_string();
    assert!(
        route.starts_with(&format!(
            "{} connections on one route, the oldest:",
            waits.len()
        )) && route.contains(&service_bind)
            && route.contains(&adapter_bind)
            && route.contains(&format!("held by the client at {own_address}"))
            && route.contains(&format!("lsof -nP -i :{own_port}")),
        "the blocker counts the held connections and names the oldest, its adapter, the \
         endpoint and the client holding it: {route}"
    );
    deployment.report["held_route_blocker"] = json!(route);
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

    // A client that reads no waiting list - a curl in a hook, a program on
    // another machine - is refused by the adapter itself, at once, with the
    // same sentence as an HTTP 502, instead of being relayed behind the held
    // one. The resolver says so before anything is read from that client.
    let mut second = TcpStream::connect(&adapter_bind).unwrap();
    let said = deployment.await_line("refused connection: not relayed:");
    assert!(
        said.contains("holds") && said.contains("unanswered longer than"),
        "the resolver names the held wait the new connection would stand behind: {said}"
    );
    let mut answer = String::new();
    second.read_to_string(&mut answer).unwrap();
    assert!(
        answer.starts_with("HTTP/1.1 502") && answer.contains("not relayed:"),
        "the second client is told why, as HTTP: {answer}"
    );
    deployment.report["second_client_refusal"] = json!(answer.trim());

    // A remote command to the host whose service stands says the held
    // reading beside itself: the command channel is a new connection, not
    // the resolver's session. This fixture's target declares no key the
    // channel can read, so the command fails there, and the refusal names
    // the held wait before the key failure instead of the vault's error
    // alone. Sent from a machine that is not the target, so the command
    // would ride the channel: this machine reads as another host for this
    // one call.
    let command = deployment.from_elsewhere(&["host", "uptime", TARGET]);
    let stderr = String::from_utf8_lossy(&command.stderr);
    assert!(
        !command.status.success(),
        "a command whose channel key cannot be read fails: {stderr}"
    );
    assert!(
        stderr.contains(&format!("connection(s) to {TARGET} unanswered"))
            && stderr.contains(&format!("services {SERVICE}"))
            && stderr.contains(HELD_SENTENCE)
            && stderr.contains(&service_bind)
            && stderr.contains("cannot be read"),
        "the refusal names the host, its standing service, the held wait and the key: {stderr}"
    );
    deployment.report["remote_command_refusal"] = json!(stderr.trim());
    deployment.report["outcome"] = json!("passed");
    deployment.save();
}
