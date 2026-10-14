//! A relay that fails on its remote end says how long it ran and whether the
//! SSH session under it survived, so a service that dropped one connection is
//! told apart from a session that died under every channel.
//!
//! One isolated deployment (`held_deployment.rs`): `elsewhere-api` is placed
//! on `other-host`, reached at `tester@[<loopback>]:<port>`, where a real SSH
//! server (`session/server.rs`) accepts every forwarded channel and closes it
//! at once while the session lives on. The real `stado serve --resolver
//! --target marker-host` relays an adapter for it. A client on the adapter
//! reads the end the closed channel gave it and then sends its request into
//! that channel; the resolver's log must name the failed relay, its elapsed
//! milliseconds and that the session to `other-host` is still open.

use serde_json::{json, Value};
use std::fs;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::process::Command;

use crate::held_deployment::{Deployment, CONSUMER, TARGET};
use crate::session::server;

const ELSEWHERE: &str = "elsewhere-api";
const ELSEWHERE_HOST: &str = "other-host";

#[test]
fn a_channel_the_service_end_closed_is_named_apart_from_a_dead_session() {
    let mut deployment = Deployment::start();
    deployment.cli(&["config", "init"]);
    let hostname = Command::new("hostname").output().unwrap();
    let hostname = String::from_utf8(hostname.stdout)
        .unwrap()
        .trim()
        .to_lowercase();

    let ssh_dir = deployment.root.join(".ssh");
    fs::create_dir_all(&ssh_dir).unwrap();
    let host_key = server::key_pair(&deployment.root.join("ssh-host-key"));
    fs::create_dir_all(deployment.root.join(".stado")).unwrap();
    server::key_pair(&deployment.root.join(".stado").join("resolver-ssh-key"));
    let ssh_bind = deployment.free_bind();
    server::serve(&ssh_bind, &host_key, server::Mode::CloseEveryChannel);
    let (ssh_host, ssh_port) = ssh_bind.rsplit_once(':').unwrap();
    let host_public = fs::read_to_string(host_key.with_extension("pub")).unwrap();
    fs::write(
        ssh_dir.join("known_hosts"),
        format!("[{ssh_host}]:{ssh_port} {host_public}"),
    )
    .unwrap();
    let destination = format!("tester@[{ssh_host}]:{ssh_port}");

    // The endpoint the directory names on the other host; the server closes
    // every channel before anything is forwarded to it.
    let endpoint = deployment.free_bind();
    let adapter_bind = deployment.free_bind();
    let api_bind = deployment.free_bind();
    let mut document: Value = serde_json::from_str(include_str!("../../registry.json")).unwrap();
    for target in document["targets"].as_array_mut().unwrap().iter_mut() {
        match target["name"].as_str() {
            Some(TARGET) => {
                target["hostnames"] = json!([hostname]);
                target["service_resolver"]["api_bind"] = json!(api_bind);
                target["service_resolver"]["adapters"] =
                    json!([{"service": ELSEWHERE, "consumer": CONSUMER, "bind": adapter_bind}]);
            }
            Some(ELSEWHERE_HOST) => target["ssh"] = json!(destination),
            _ => {}
        }
    }
    document["service_directory"]["services"][ELSEWHERE]["endpoints"][ELSEWHERE_HOST]["url"] =
        json!(format!("http://{endpoint}"));
    let registry = deployment.root.join("registry.json");
    fs::write(&registry, serde_json::to_vec_pretty(&document).unwrap()).unwrap();
    let registry = registry.to_string_lossy().into_owned();
    deployment.cli(&["registry", "import", &registry]);

    deployment.serve();
    deployment.await_line(&format!("stado resolver target={TARGET}"));

    // The closed channel ends the download first: the client reads that end,
    // then sends its request into the channel that is gone.
    let mut client = TcpStream::connect(&adapter_bind).unwrap();
    let mut before = Vec::new();
    client.read_to_end(&mut before).unwrap();
    deployment.report["read_before_request"] = json!(String::from_utf8_lossy(&before));
    let _ = client.write_all(b"GET /dropped HTTP/1.1\r\nHost: elsewhere-api\r\n\r\n");
    let _ = client.flush();

    let said = deployment.await_line(&format!(
        "stado resolver adapter service={ELSEWHERE} consumer={CONSUMER} rejected connection"
    ));
    deployment.report["rejected"] = json!(said);
    assert!(
        said.contains(" ms; the SSH session to ")
            && said.contains(ELSEWHERE_HOST)
            && said.contains("still open, so the service end closed this one channel"),
        "the failed relay names its elapsed time and the surviving session: {said}"
    );
    deployment.report["outcome"] = json!("passed");
    deployment.save();
}
