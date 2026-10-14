//! A resolver whose registry authority accepts the SSH connection and
//! answers nothing serves the registry its store already holds and binds
//! its adapters, instead of standing in state `starting` with no listener
//! for as long as the authority stays silent.
//!
//! One isolated deployment (`held_deployment.rs`): the imported registry
//! names `other-host` as the service directory's authority, reached at a
//! loopback listener this test owns that accepts every connection and sends
//! nothing, so the SSH client waits for a banner that never comes. The real
//! `stado serve --resolver --target marker-host` must log that the
//! authority's first snapshot did not answer, bind the adapter declared for
//! `marker-api`, publish `serving` with that adapter listening, and relay a
//! client through it to a service that answers. The test holds no clock:
//! the resolver must give up the first read by the refresh interval the
//! registry declares, on its own.
use serde_json::{json, Value};
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::Command;

use crate::held_deployment::{Deployment, CONSUMER, SERVICE, TARGET};

const AUTHORITY: &str = "other-host";
const AUTHORITY_STADO: &str = "/usr/local/bin/stado";
const ANSWER: &str = "served from the registry store";

/// A listener that accepts and never speaks, keeping every connection open;
/// its SSH destination, written `[host]:port` as a registry path names a
/// server off SSH's own port.
fn silent_authority(deployment: &mut Deployment) -> String {
    let listener = TcpListener::bind(deployment.free_bind()).unwrap();
    let address = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        let mut held = Vec::new();
        for accepted in listener.incoming() {
            let Ok(stream) = accepted else { return };
            held.push(stream);
        }
    });
    format!("[{}]:{}", address.ip(), address.port())
}

/// A service that answers every request with `ANSWER` and closes.
fn answering_service(deployment: &mut Deployment) -> String {
    let listener = TcpListener::bind(deployment.free_bind()).unwrap();
    let address = listener.local_addr().unwrap().to_string();
    std::thread::spawn(move || {
        for accepted in listener.incoming() {
            let Ok(mut stream) = accepted else { return };
            let request = BufReader::new(stream.try_clone().unwrap());
            for line in request.lines() {
                match line {
                    Ok(line) if !line.is_empty() => continue,
                    _ => break,
                }
            }
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{ANSWER}",
                ANSWER.len()
            );
        }
    });
    address
}

#[test]
fn a_silent_authority_does_not_keep_the_resolver_from_serving_its_store() {
    let mut deployment = Deployment::start();
    deployment.cli(&["config", "init"]);
    let hostname = Command::new("hostname").output().unwrap();
    let hostname = String::from_utf8(hostname.stdout)
        .unwrap()
        .trim()
        .to_lowercase();
    let authority_ssh = silent_authority(&mut deployment);
    let service_bind = answering_service(&mut deployment);
    let adapter_bind = deployment.free_bind();
    let api_bind = deployment.free_bind();
    let mut document: Value = serde_json::from_str(include_str!("../../registry.json")).unwrap();
    let mut refresh_seconds = None;
    for target in document["targets"].as_array_mut().unwrap().iter_mut() {
        match target["name"].as_str() {
            Some(TARGET) => {
                target["hostnames"] = json!([hostname]);
                target["service_resolver"]["api_bind"] = json!(api_bind);
                target["service_resolver"]["adapters"] =
                    json!([{"service": SERVICE, "consumer": CONSUMER, "bind": adapter_bind}]);
                refresh_seconds = target["service_resolver"]["refresh_seconds"].as_i64();
            }
            Some(AUTHORITY) => target["ssh"] = json!(authority_ssh),
            _ => {}
        }
    }
    let refresh_seconds = refresh_seconds.expect("the registry declares a refresh interval");
    document["service_directory"]["authority"] =
        json!({"target": AUTHORITY, "command": AUTHORITY_STADO});
    document["service_directory"]["services"][SERVICE]["endpoints"][TARGET]["url"] =
        json!(format!("http://{service_bind}"));
    let registry = deployment.root.join("registry.json");
    fs::write(&registry, serde_json::to_vec_pretty(&document).unwrap()).unwrap();
    let registry = registry.to_string_lossy().into_owned();
    deployment.cli(&["registry", "import", &registry]);

    deployment.serve();
    let said = deployment.await_line("the registry authority's first snapshot did not answer");
    assert!(
        said.contains(AUTHORITY)
            && said.contains(&format!("{refresh_seconds}s refresh interval"))
            && said.contains("serving generation")
            && said.contains("from the registry store immediately"),
        "the resolver names the silent authority, the interval it gave the read and the generation it serves instead: {said}"
    );
    deployment.await_line(&format!("stado resolver target={TARGET}"));

    let output = deployment.run(&["resolver", "status", "--json"]);
    let status: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(status["state"], "serving", "{status}");
    let bound = status["adapters"]
        .as_array()
        .into_iter()
        .flatten()
        .any(|adapter| adapter["bind"] == adapter_bind && adapter["listening"] == true);
    assert!(bound, "the adapter is bound and listening: {status}");

    let mut client = TcpStream::connect(&adapter_bind).unwrap();
    client
        .write_all(b"GET /through HTTP/1.1\r\nHost: marker-api\r\n\r\n")
        .unwrap();
    client.flush().unwrap();
    let mut answer = String::new();
    client.read_to_string(&mut answer).unwrap();
    assert!(
        answer.contains(ANSWER),
        "a client is relayed through the adapter to the service: {answer}"
    );
    deployment.report["startup"] = json!(said);
    deployment.report["status"] = status;

    // The resolver ends as a crash or an install ends it; the state it
    // published stays on disk saying `serving`, and a reader that trusted
    // it sent traffic to ports nobody holds. `resolver status` must read
    // the file as the word of a process that is gone.
    let pid = deployment.end_resolver();
    let output = deployment.run(&["resolver", "status", "--json"]);
    let after: Value = serde_json::from_slice(&output.stdout).unwrap();
    let gone = after["blockers"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .find(|blocker| blocker.contains("is not running"))
        .map(str::to_owned);
    let Some(gone) = gone else {
        panic!("status names the exited publisher among its blockers: {after}");
    };
    assert!(
        gone.contains(&format!("(pid {pid}, state serving"))
            && gone.contains("no resolver has published since"),
        "the blocker names the exited process and its state: {gone}"
    );
    assert!(
        !output.status.success(),
        "status is not ready while the published state is a dead process's"
    );
    deployment.report["after_exit"] = after;
    deployment.report["outcome"] = json!("passed");
    deployment.save();
}
