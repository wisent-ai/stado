//! A route whose endpoint on this host is one of this resolver's own
//! listeners is refused to the client at once, naming the route and the
//! listener, instead of being relayed back into the same process - where the
//! connection would be accepted, forwarded to the same socket, accepted
//! again, and held forever with nothing to answer it.
//!
//! One isolated deployment (`held_deployment.rs`): a local store holding
//! `registry.json`, whose `marker-api` route is placed on `marker-host` at
//! the very address the host's adapter for it binds, and the real `stado
//! serve --resolver --target marker-host`. A client connects to the adapter
//! and reads to the end: it must receive an HTTP 502 whose body names the
//! service, the loop and this host, the resolver must say the same on its
//! log, and the resolver must still be serving afterwards (a second client on
//! the same adapter is answered the same way, not held).
use serde_json::{json, Value};
use std::fs;
use std::io::Read;
use std::net::TcpStream;
use std::process::Command;

mod held_deployment;
use held_deployment::{Deployment, CONSUMER, SERVICE, TARGET};

const LOOP_SENTENCE: &str = "relaying it would loop back into this process";

/// One client connecting to `adapter_bind` and reading its whole answer.
fn answer_of(adapter_bind: &str) -> String {
    let mut client = TcpStream::connect(adapter_bind).unwrap();
    let mut answer = String::new();
    client.read_to_string(&mut answer).unwrap();
    answer
}

#[test]
fn a_route_at_the_resolvers_own_adapter_is_refused_to_the_client_not_relayed_into_itself() {
    let mut deployment = Deployment::start();
    deployment.cli(&["config", "init"]);
    let hostname = Command::new("hostname").output().unwrap();
    let hostname = String::from_utf8(hostname.stdout)
        .unwrap()
        .trim()
        .to_lowercase();
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
    // The loop: the route's endpoint on this host is the adapter's own bind.
    document["service_directory"]["services"][SERVICE]["endpoints"][TARGET]["url"] =
        json!(format!("http://{adapter_bind}"));
    let registry = deployment.root.join("registry.json");
    fs::write(&registry, serde_json::to_vec_pretty(&document).unwrap()).unwrap();
    let registry = registry.to_string_lossy().into_owned();
    deployment.cli(&["registry", "import", &registry]);

    deployment.serve();
    deployment.await_line(&format!("stado resolver target={TARGET}"));

    let answer = answer_of(&adapter_bind);
    assert!(
        answer.starts_with("HTTP/1.1 502")
            && answer.contains(LOOP_SENTENCE)
            && answer.contains(&format!(
                "service {SERVICE} resolves to http://{adapter_bind}"
            ))
            && answer.contains(TARGET),
        "the client is told the route loops into this resolver: {answer}"
    );
    let said = deployment.await_line("refused connection:");
    assert!(
        said.contains(LOOP_SENTENCE) && said.contains(SERVICE),
        "the resolver names the loop on its log: {said}"
    );
    deployment.report["first_answer"] = json!(answer.trim());

    // The resolver is still serving: a second client is answered the same
    // way at once, and nothing stands in the waiting list.
    let again = answer_of(&adapter_bind);
    assert!(
        again.starts_with("HTTP/1.1 502") && again.contains(LOOP_SENTENCE),
        "the second client is refused the same way: {again}"
    );
    assert!(
        deployment.published_waits().is_empty(),
        "a refused loop holds no place in the waiting list"
    );
    deployment.report["second_answer"] = json!(again.trim());
    deployment.report["outcome"] = json!("passed");
    deployment.save();
}
