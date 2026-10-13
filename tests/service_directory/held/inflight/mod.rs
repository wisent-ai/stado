//! A registry read sent before the route stood held does not stand with it
//! without end: once this host's resolver publishes that route held longer
//! than the refresh interval it declares, the read is abandoned and refused
//! naming the adapter address, with a non-zero exit.
//!
//! One isolated deployment (`held_deployment.rs`): a local store holding
//! `registry.json`, which places `marker-api` on `marker-host` at a loopback
//! address the test listens on and never answers, and the real `stado serve
//! --resolver --target marker-host` relaying the adapter declared for it.
//! Nothing is held when `stado registry pull --generation-only`, whose store
//! is that adapter, starts: its request is sent and goes up the channel to
//! the silent service, and the resolver publishes it waiting for an answer.
//! The command must end on its own — no clock of the test's — with the
//! abandonment sentence naming the adapter, and the published wait must be
//! gone with it, because the dropped request closed its connection.
use serde_json::{json, Value};
use std::fs;
use std::process::{Command, Stdio};

use crate::held_deployment::{Deployment, CONSUMER, SERVICE, TARGET};

const ABANDONED: &str = "request abandoned unanswered";
const HELD_SENTENCE: &str = "for the first byte of an answer on an open channel";

#[test]
fn a_registry_read_sent_before_the_route_stood_is_abandoned_naming_the_adapter() {
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
    let mut document: Value = serde_json::from_str(include_str!("../../registry.json")).unwrap();
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
    assert!(
        deployment.published_waits().is_empty(),
        "nothing stands held before the read is sent"
    );

    // The read is sent while nothing is held, so it goes up the channel and
    // the silent service holds it; the resolver publishes the wait.
    let args = ["registry", "pull", "--generation-only"];
    let child = deployment
        .adapter_command()
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let said = deployment.await_line("now waiting for an answer");
    assert!(
        said.contains(&format!("sent to {service_bind}")),
        "the resolver names the relayed request: {said}"
    );
    let waits = deployment.published_waits();
    let [wait] = waits.as_slice() else {
        panic!("exactly one relayed request waits: {waits:?}");
    };
    assert_eq!(wait["bind"], adapter_bind);

    // The command ends on its own once its wait has passed the declared
    // refresh interval: it is not told, and the test holds no clock.
    let output = child.wait_with_output().unwrap();
    deployment.record(&args, &output);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success(),
        "a read abandoned on a held route fails: {stderr}"
    );
    assert!(
        stderr.contains(ABANDONED)
            && stderr.contains(&adapter_bind)
            && stderr.contains(HELD_SENTENCE)
            && stderr.contains(&format!("{refresh_seconds}s refresh interval")),
        "the refusal names the abandoned request, the adapter it stood on, the held wait and the \
         interval that measured it: {stderr}"
    );

    // The dropped request closed its connection, so the resolver no longer
    // holds it: the next reader is not refused on its account.
    deployment.await_line("abandoned by its client before an answer");
    assert!(
        deployment.published_waits().is_empty(),
        "the abandoned request's wait is gone from the published list"
    );
    deployment.report["abandonment"] = json!(stderr.trim());
    deployment.report["outcome"] = json!("passed");
    deployment.save();
}
