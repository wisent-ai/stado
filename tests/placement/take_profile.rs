//! `stado placement take`: a placement profile taken on this host from the
//! host it is placed on, which answers on no SSH path, run against an
//! isolated deployment's own registry and the built `stado`.
//!
//! The template places `elsewhere-api` on `other-host`, whose only SSH path
//! is `other-host.invalid`: a name that never resolves (RFC 6761 §6.4), so
//! the source is a host no SSH path reaches, reached the way a real one is.
//! The deployment runs as `marker-host`; its resolver's refresh interval,
//! the bound every answer is given, is published in its own resolver state.
//! Each refusal leaves the registry as it was:
//!
//! - a profile that names no units on this host is refused, naming the host;
//! - a source whose SSH is closed but whose service still answers (the
//!   source declares `localhost` among its names, and a listener here holds
//!   the probe's port and answers HTTP) is refused naming the service and
//!   the URL that answered, before any state is read;
//! - with nothing listening there, a profile whose required state this host
//!   does not hold is refused naming the state;
//! - a request naming no service is a usage error.
//!
//! A take that commits needs a unit that starts and answers on this host,
//! which this isolated deployment does not run; the take on the operator's
//! fleet is its live qualification.

use std::io::{Read, Write};
use std::net::TcpListener;

use serde_json::{json, Value};

use crate::deployment::{profile, Deployment, PLACED_ON, PROFILE, SERVICE};

/// The host the isolated deployment runs as: the template's first target,
/// whose hostnames the deployment sets to this machine's.
const HERE: &str = "marker-host";
/// The resolver refresh interval the deployment publishes, in seconds: the
/// bound a probe of the source is given, declared here as a resolver
/// publishes it.
const REFRESH_SECONDS: u64 = 2;

/// The deployment with `PROFILE` declared on `other-host`, known also as
/// `localhost`, whose probe asks `port`; with `here_state`, declared on this
/// host too and requiring that state.
fn declared(port: u16, here_state: Option<&str>) -> Deployment {
    let deployment = Deployment::start();
    deployment.cli(&["config", "init"]);
    deployment.import_registry();
    let mut registry = deployment.registry();
    let source = registry["targets"]
        .as_array_mut()
        .and_then(|targets| {
            targets
                .iter_mut()
                .find(|target| target["name"] == PLACED_ON)
        })
        .expect("the template registers other-host");
    source["hostnames"] = json!(["localhost"]);
    deployment.import(&registry);
    let mut declared = profile(&deployment.registry(), PROFILE, &[SERVICE]);
    declared["hosts"][PLACED_ON]["probes"] =
        json!([{ "service": SERVICE, "url": format!("http://127.0.0.1:{port}/health") }]);
    if let Some(state) = here_state {
        let placed = declared["hosts"][PLACED_ON].clone();
        declared["hosts"][HERE] = placed;
        declared["state"] = json!([{ "path": state, "required": true }]);
    }
    let file = deployment.write_profile("profile.json", &declared);
    deployment.cli(&["placement", "profile", "declare", "--file", &file]);
    let published = json!({ "refresh_seconds": REFRESH_SECONDS });
    let state = deployment.root.join(".stado").join("resolver-state.json");
    std::fs::write(state, serde_json::to_vec(&published).unwrap()).unwrap();
    deployment
}

/// A port nothing listens on: one the kernel handed out, then closed.
fn closed_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.local_addr().unwrap().port()
}

#[test]
fn a_profile_is_not_taken_onto_a_host_it_names_no_units_for() {
    let deployment = declared(closed_port(), None);
    for check in [true, false] {
        let mut args = vec!["placement", "take", SERVICE, "--reason", "journey"];
        if check {
            args.push("--check");
        }
        deployment.refused(&args, HERE);
    }
}

#[test]
fn a_source_that_still_serves_is_not_taken_from() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let deployment = declared(port, Some(".stado/journey-take-state.json"));
    // Answer every request the take sends while the journey runs.
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let mut stream = stream;
            let mut request = [0u8; 1024];
            let _ = stream.read(&mut request);
            let _ = stream.write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\n\r\nok");
        }
    });
    let url = format!("http://localhost:{port}/health");
    for check in [true, false] {
        let mut args = vec!["placement", "take", SERVICE, "--reason", "journey"];
        if check {
            args.push("--check");
        }
        deployment.refused(
            &args,
            &format!("{PLACED_ON} still serves {SERVICE} at {url}"),
        );
    }
}

#[test]
fn a_profile_is_not_taken_without_the_state_it_requires() {
    let deployment = declared(closed_port(), Some(".stado/journey-take-state.json"));
    deployment.refused(
        &["placement", "take", SERVICE, "--reason", "journey"],
        "holds none of",
    );
    let output = deployment.run(&[
        "placement",
        "take",
        SERVICE,
        "--reason",
        "journey",
        "--check",
    ]);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success(),
        "a check without the required state passed"
    );
    assert!(
        stderr.contains("journey-take-state.json"),
        "the refusal names the state: {stderr}"
    );
    assert!(
        stderr.contains(PLACED_ON),
        "the refusal names the silent source: {stderr}"
    );
}

#[test]
fn a_take_names_the_profile_it_takes() {
    let deployment = declared(closed_port(), None);
    let output = deployment.run(&["placement", "take", "--reason", "journey"]);
    assert!(!output.status.success(), "a take naming no service ran");
    let receipt: Option<Value> = serde_json::from_slice(&output.stdout).ok();
    assert!(receipt.is_none(), "a usage error printed a receipt");
}
