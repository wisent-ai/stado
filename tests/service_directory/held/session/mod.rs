//! A route held on the resolver's own SSH session is carried by a new
//! session to the same host, instead of every later client being refused
//! behind the held one.
//!
//! One isolated deployment (`held_deployment.rs`): `elsewhere-api` is placed
//! on `other-host`, reached at `tester@[<loopback>]:<port>` (the known_hosts
//! form for a server off SSH's own port), where a real SSH server
//! (`server.rs`) runs whose first carrying session accepts channels and
//! forwards nothing, while every other session forwards to a service that
//! answers. The real `stado serve --resolver --target marker-host` relays an
//! adapter for it. A first client's request stands on the resolver's session;
//! once it has waited past the declared refresh interval the resolver must
//! replace that session on its own, saying so on its log, before any other
//! client arrives; a second client on the same adapter must then be relayed
//! over the new session and answered, and the held wait must carry
//! `session_replaced_at` in the published list and in `resolver status`.

mod server;

use serde_json::{json, Value};
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::process::Command;

use crate::held_deployment::{Deployment, CONSUMER, TARGET};

const ELSEWHERE: &str = "elsewhere-api";
const ELSEWHERE_HOST: &str = "other-host";
const ANSWER: &str = "carried by a new session";

/// A service that answers every request with `ANSWER` and closes, at an
/// address this machine handed out.
fn answering_service(deployment: &mut Deployment) -> SocketAddr {
    let listener = TcpListener::bind(deployment.free_bind()).unwrap();
    let address = listener.local_addr().unwrap();
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

/// The resolver's status, read once.
fn status(deployment: &mut Deployment) -> Value {
    let output = deployment.run(&["resolver", "status", "--json"]);
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn a_route_held_on_the_resolvers_session_is_carried_by_a_new_session() {
    let mut deployment = Deployment::start();
    deployment.cli(&["config", "init"]);
    let hostname = Command::new("hostname").output().unwrap();
    let hostname = String::from_utf8(hostname.stdout)
        .unwrap()
        .trim()
        .to_lowercase();

    // The host's SSH server, trusted under the name known_hosts gives a
    // server off SSH's own port, and the resolver's own identity.
    let ssh_dir = deployment.root.join(".ssh");
    fs::create_dir_all(&ssh_dir).unwrap();
    let host_key = server::key_pair(&deployment.root.join("ssh-host-key"));
    fs::create_dir_all(deployment.root.join(".stado")).unwrap();
    server::key_pair(&deployment.root.join(".stado").join("resolver-ssh-key"));
    let service = answering_service(&mut deployment);
    let ssh_bind = deployment.free_bind();
    server::serve(&ssh_bind, &host_key, service);
    let (ssh_host, ssh_port) = ssh_bind.rsplit_once(':').unwrap();
    let host_public = fs::read_to_string(host_key.with_extension("pub")).unwrap();
    fs::write(
        ssh_dir.join("known_hosts"),
        format!("[{ssh_host}]:{ssh_port} {host_public}"),
    )
    .unwrap();
    let destination = format!("tester@[{ssh_host}]:{ssh_port}");

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
                    json!([{"service": ELSEWHERE, "consumer": CONSUMER, "bind": adapter_bind}]);
                refresh_seconds = target["service_resolver"]["refresh_seconds"].as_i64();
            }
            Some(ELSEWHERE_HOST) => target["ssh"] = json!(destination),
            _ => {}
        }
    }
    let refresh_seconds = refresh_seconds.expect("the registry declares a refresh interval");
    document["service_directory"]["services"][ELSEWHERE]["endpoints"][ELSEWHERE_HOST]["url"] =
        json!(format!("http://{service}"));
    let registry = deployment.root.join("registry.json");
    fs::write(&registry, serde_json::to_vec_pretty(&document).unwrap()).unwrap();
    let registry = registry.to_string_lossy().into_owned();
    deployment.cli(&["registry", "import", &registry]);

    deployment.serve();
    deployment.await_line(&format!("stado resolver target={TARGET}"));

    // The first request goes up the resolver's session, which stands.
    let mut held = TcpStream::connect(&adapter_bind).unwrap();
    held.write_all(b"GET /held HTTP/1.1\r\nHost: elsewhere-api\r\n\r\n")
        .unwrap();
    held.flush().unwrap();
    deployment.await_line("now waiting for an answer");

    // Past the refresh interval the resolver replaces the standing session on
    // its own: a caller that reads the waiting list refuses itself before it
    // connects, so no connection would ever arrive to replace it.
    let said = deployment.await_line(&format!("while a new session to {ELSEWHERE_HOST} answered"));
    assert!(
        said.contains(&format!("service={ELSEWHERE}")) && said.contains("stands"),
        "the resolver names the route whose session it replaced: {said}"
    );
    deployment.report["renewal"] = json!(said);

    // A later client on the same adapter rides the new session and is answered.
    let mut carried = TcpStream::connect(&adapter_bind).unwrap();
    carried
        .write_all(b"GET /carried HTTP/1.1\r\nHost: elsewhere-api\r\n\r\n")
        .unwrap();
    carried.flush().unwrap();
    let mut answer = String::new();
    carried.read_to_string(&mut answer).unwrap();
    assert!(
        answer.starts_with("HTTP/1.1 200") && answer.ends_with(ANSWER),
        "the later client is answered over the new session: {answer}"
    );
    deployment.report["carried_answer"] = json!(answer);

    let waits = deployment.published_waits();
    let replaced: Vec<&Value> = waits
        .iter()
        .filter(|wait| wait["session_replaced_at"].is_string())
        .collect();
    assert!(
        !replaced.is_empty() && replaced.iter().all(|wait| wait["bind"] == adapter_bind),
        "the held wait names the replaced session: {waits:?}"
    );
    let report = status(&mut deployment);
    let replaced_open = report["waiting_opens"].as_array().and_then(|opens| {
        opens
            .iter()
            .find(|open| open["session_replaced_at"].is_string())
    });
    assert!(
        replaced_open
            .and_then(|open| open["waited_seconds"].as_i64())
            .is_some_and(|seconds| seconds > refresh_seconds),
        "the replaced wait stood past the declared refresh interval: {report}"
    );
    let blockers: Vec<String> = report["blockers"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect();
    assert!(
        blockers
            .iter()
            .any(|blocker| blocker.contains("the SSH session it rides was replaced at")),
        "resolver status names the replaced session: {blockers:?}"
    );
    deployment.report["replaced_session_blockers"] = json!(blockers);
    drop(held);
    deployment.report["outcome"] = json!("passed");
    deployment.save();
}
