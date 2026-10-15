//! `stado placement move --authority` takes the registry authority role
//! from an authority that accepts the SSH connection and answers nothing,
//! and the host's own resolver follows it.
//!
//! One isolated deployment (`held_deployment.rs`): the registry names
//! `other-host` as authority, reached at a loopback listener that never
//! speaks. This host is `marker-host`, whose last-known-good copy holds the
//! registry its resolver read. The takeover run anywhere but on the host
//! that takes the role is refused; run on `marker-host` it promotes the
//! copy above the generation the old authority served, writes it into the
//! store and the copy, and the running resolver logs that it adopted it.
//! The store then holds the promoted document, naming `marker-host`.
use serde_json::{json, Value};
use std::fs;
use std::net::TcpListener;
use std::process::Command;

use crate::held_deployment::{Deployment, TARGET};

const AUTHORITY: &str = "other-host";
const AUTHORITY_STADO: &str = "/usr/local/bin/stado";

/// A listener that accepts and never speaks, keeping every connection open.
fn silent_authority(deployment: &mut Deployment) -> String {
    let listener = TcpListener::bind(deployment.free_bind()).unwrap();
    let address = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        let mut held = Vec::new();
        for stream in listener.incoming().flatten() {
            held.push(stream);
        }
    });
    format!("[{}]:{}", address.ip(), address.port())
}

fn document_of(output: &std::process::Output) -> Value {
    match serde_json::from_slice(&output.stdout) {
        Ok(document) => document,
        Err(error) => panic!(
            "stado printed no JSON document ({error}): {} / {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        ),
    }
}

#[test]
fn a_silent_authority_is_replaced_by_the_host_that_takes_the_role() {
    let mut deployment = Deployment::start();
    deployment.cli(&["config", "init"]);
    let hostname = Command::new("hostname").output().unwrap();
    let hostname = String::from_utf8(hostname.stdout)
        .unwrap()
        .trim()
        .to_lowercase();
    let authority_ssh = silent_authority(&mut deployment);
    let api_bind = deployment.free_bind();
    let mut document: Value = serde_json::from_str(include_str!("../../registry.json")).unwrap();
    for target in document["targets"].as_array_mut().unwrap().iter_mut() {
        match target["name"].as_str() {
            Some(TARGET) => {
                target["hostnames"] = json!([hostname]);
                target["service_resolver"]["api_bind"] = json!(api_bind);
            }
            Some(AUTHORITY) => target["ssh"] = json!(authority_ssh),
            _ => {}
        }
    }
    document["service_directory"]["authority"] =
        json!({"target": AUTHORITY, "command": AUTHORITY_STADO});
    let served = document["service_directory"]["generation"]
        .as_u64()
        .expect("the fixture declares a directory generation");
    let registry = deployment.root.join("registry.json");
    fs::write(&registry, serde_json::to_vec_pretty(&document).unwrap()).unwrap();
    let registry = registry.to_string_lossy().into_owned();
    deployment.cli(&["registry", "import", &registry]);
    // What this host's resolver keeps after it last read the authority.
    let cache = deployment.root.join(".stado").join("cache");
    fs::create_dir_all(&cache).unwrap();
    fs::write(
        cache.join("registry-last-good.json"),
        serde_json::to_vec_pretty(&document).unwrap(),
    )
    .unwrap();

    deployment.serve();
    deployment.await_line("the registry authority's first snapshot did not answer");

    let elsewhere = deployment.run(&["placement", "move", "--authority", "--to-host", AUTHORITY]);
    let refusal = String::from_utf8_lossy(&elsewhere.stderr).into_owned();
    assert!(
        !elsewhere.status.success()
            && refusal.contains(&format!("run this on {AUTHORITY}, not {TARGET}")),
        "the role is taken only on the host that takes it: {refusal}"
    );

    let taken = deployment.run(&[
        "placement",
        "move",
        "--authority",
        "--to-host",
        TARGET,
        "--json",
    ]);
    let receipt = document_of(&taken);
    assert!(taken.status.success(), "the takeover completes: {receipt}");
    assert_eq!(receipt["state"], "taken", "{receipt}");
    assert_eq!(receipt["authority"], TARGET, "{receipt}");
    assert_eq!(receipt["fenced"], AUTHORITY, "{receipt}");
    assert!(receipt["store"]["version"].is_string(), "{receipt}");
    assert!(
        receipt["authority_unanswered"].is_string(),
        "the receipt carries the silent authority's refusal: {receipt}"
    );
    let promoted = receipt["directory_generation"]
        .as_u64()
        .expect("the receipt names the promoted generation");
    assert!(
        promoted > served,
        "the promoted generation {promoted} fences the {served} the old authority served"
    );

    let adopted = deployment.await_line("adopting the newer registry this host was handed");
    assert!(
        adopted.contains(&format!("directory generation {promoted}")),
        "the resolver adopts the promoted generation: {adopted}"
    );

    let pulled = deployment.run(&["registry", "pull"]);
    let stored = document_of(&pulled);
    assert_eq!(
        stored["service_directory"]["authority"]["target"], TARGET,
        "the store names the new authority: {stored}"
    );
    assert_eq!(
        stored["service_directory"]["generation"], promoted,
        "the store carries the promoted generation: {stored}"
    );
    deployment.report["receipt"] = receipt;
    deployment.report["adopted"] = json!(adopted);
    deployment.report["outcome"] = json!("passed");
    deployment.save();
}
