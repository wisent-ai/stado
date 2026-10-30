//! A host whose worker cannot reach the fleet store says so in `stado host
//! gates`, not only "nothing published".
//!
//! One isolated deployment whose registry names this machine: `host gates`
//! first finds no capacity publication and no record of an attempt, and says
//! the worker has not run under a Stado that keeps one. Then the real `stado
//! serve --worker --standalone` runs with its fleet store at a loopback port
//! the product allocated and nothing listens on; its worker records the
//! refusal in `~/.stado/logs/capacity-publication.json`. With the worker
//! stopped and the control plane reading its own local store again, `host
//! gates` reads that record over the host channel and names the refusal, its
//! step, its first moment and the store's own words.
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::time::{Duration, Instant};

use crate::deployment::{line_with, Deployment};
use serde_json::{json, Value};

#[test]
fn host_gates_names_a_worker_that_cannot_reach_the_fleet_store() {
    let mut deployment = Deployment::start("publication-attempt");
    let store = deployment.store();
    let registry: Value =
        serde_json::from_slice(&fs::read(store.join("registry.json")).unwrap()).unwrap();
    let Some(host) = registry["targets"]
        .as_array()
        .and_then(|targets| targets.first())
        .and_then(|target| target["name"].as_str())
        .map(str::to_string)
    else {
        panic!("config init must seed a registry naming this machine: {registry}");
    };
    let local = store.to_string_lossy().into_owned();
    deployment.set("WC_STORAGE_BACKEND", "local");
    deployment.set("WC_LOCAL_STORAGE_PATH", &local);

    // No worker ever ran here: no publication and no record of an attempt.
    let (_, before, stderr) = deployment.run(&["host", "gates", &host], "");
    assert!(
        before.contains(
            "capacity: nothing published for this host, so the scheduler cannot see it; the \
             account this command reaches it as keeps no record of a publication attempt \
             (~/.stado/logs/capacity-publication.json): its worker has not run under a Stado \
             that keeps one (upgrade it, or start its worker), or it runs with another home \
             than this account's (a unit whose environment sets no HOME; set HOME in the unit)"
        ),
        "{before}{stderr}"
    );

    // The worker's fleet store is a port nothing listens on.
    let port = deployment
        .cli(&["host", "local", "port"])
        .trim()
        .parse::<u16>()
        .expect("the product returns an allocated local port");
    let unanswered = format!("http://{}:{port}", std::net::Ipv4Addr::LOCALHOST);
    let token = deployment.root.join("object-api.token");
    fs::write(&token, uuid::Uuid::new_v4().to_string()).unwrap();
    fs::set_permissions(
        &token,
        fs::Permissions::from_mode(stado::primitives::file_mode::owner_read_write()),
    )
    .unwrap();
    deployment.set("WC_STORAGE_BACKEND", "stado");
    deployment.set("WC_STADO_STORAGE_URL", &unanswered);
    deployment.set("WC_STADO_STORAGE_NAMESPACE", "publication-attempt");
    deployment.set("WC_STADO_STORAGE_TOKEN_FILE", token.to_str().unwrap());
    let lines = deployment.serve(&[
        "--worker",
        "--standalone",
        "--kind",
        "local",
        "--poll-seconds",
        "1",
    ]);
    let refusal = line_with(&lines, "the fleet store did not answer");
    deployment.report["worker_refusal"] = json!(refusal);
    let record = deployment
        .root
        .join(".stado")
        .join("logs")
        .join("capacity-publication.json");
    let started = Instant::now();
    let kept: Value = loop {
        if let Ok(bytes) = fs::read(&record) {
            if let Ok(value) = serde_json::from_slice(&bytes) {
                break value;
            }
        }
        assert!(
            started.elapsed() < Duration::from_secs(60),
            "the worker kept no record of its refused attempt at {}",
            record.display()
        );
        std::thread::sleep(Duration::from_millis(200));
    };
    deployment.report["record"] = kept.clone();
    assert_eq!(kept["outcome"], json!("refused"), "{kept}");
    assert!(kept["refused_since"].is_string(), "{kept}");
    let words = kept["error"].as_str().expect("the store's words are kept");
    deployment.stop();

    // The control plane reads its own store again; the host's record says
    // why its worker publishes nothing.
    deployment.set("WC_STORAGE_BACKEND", "local");
    let (_, text, stderr) = deployment.run(&["host", "gates", &host], "");
    assert!(
        text.contains(&format!(
            "capacity: nothing published for this host, so the scheduler cannot see it; its \
             worker cannot reach the fleet store: its {} at ",
            kept["step"].as_str().unwrap()
        )) && text.contains(words),
        "{text}{stderr}"
    );
    let (_, json_text, stderr) = deployment.run(&["host", "gates", &host, "--json"], "");
    let gates: Value = serde_json::from_str(&json_text)
        .unwrap_or_else(|error| panic!("host gates --json printed no JSON ({error}): {stderr}"));
    assert_eq!(
        gates["publication_attempt"]["outcome"],
        json!("refused"),
        "{gates}"
    );
    assert_eq!(
        gates["publication_attempt"]["error"],
        json!(words),
        "{gates}"
    );
    deployment.pass();
}
