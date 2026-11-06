//! A resolver whose adapter's service is placed on a host none of whose SSH
//! paths answers refuses the next connection at once, naming that refusal,
//! instead of holding it behind another handshake that will not answer.
//!
//! The deployment (`shed/deployment.rs`) runs the real `stado serve
//! --resolver` as `marker-host` with one adapter for `elsewhere-api`, whose
//! active host is `other-host`: its only SSH path is `other-host.invalid`, a
//! name that never resolves (RFC 6761 §6.4), so every session attempt is
//! refused the way a host that went away is. The refresh interval and the
//! resolver's descriptor limit are the test's own inputs
//! (`STADO_TEST_REFRESH_SECONDS`, `STADO_TEST_FD_LIMIT`). Expected: the first
//! client reads a 502 naming the host's paths; a second client connecting
//! within the interval reads a 502 saying the host was not tried again and
//! why, sooner than the first one took; once the interval has passed, the
//! next client is tried again (its refusal names the paths, not the memory).
#[path = "shed/deployment.rs"]
// The journey takes the deployment, not the shed journey's held-wait reads.
#[allow(dead_code)]
mod deployment;

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant};

use deployment::{Deployment, SERVICE};
use serde_json::json;

const REQUEST: &[u8] = b"GET / HTTP/1.1\r\nHost: elsewhere\r\n\r\n";
/// The host the template registers with `other-host.invalid` as its only SSH path.
const SILENT: &str = "other-host";

/// A test input, refused by name when the environment does not state it.
fn input(name: &str) -> u64 {
    let value = std::env::var(name).unwrap_or_else(|error| panic!("{name} is required: {error}"));
    value
        .parse()
        .unwrap_or_else(|error| panic!("{name}={value:?} is not a count: {error}"))
}

/// One request through the adapter, its whole answer and how long it took.
fn ask(adapter_bind: &str) -> (String, Duration) {
    let started = Instant::now();
    let mut client = TcpStream::connect(adapter_bind).expect("the adapter accepts");
    client.write_all(REQUEST).expect("the request is sent");
    let mut answer = String::new();
    client
        .read_to_string(&mut answer)
        .expect("the adapter's answer reads to its close");
    (answer, started.elapsed())
}

#[test]
fn a_host_no_path_reaches_is_refused_at_once_within_the_interval() {
    let refresh = input("STADO_TEST_REFRESH_SECONDS");
    let mut deployment = Deployment::start();
    deployment.cli(&["config", "init"]);
    let api_bind = deployment.free_bind();
    let adapter_bind = deployment.free_bind();
    let service_bind = deployment.free_bind();
    deployment.import_registry(&api_bind, &adapter_bind, &service_bind, refresh);
    // Place the service on the host no path reaches, at the endpoint it
    // would serve there.
    let mut registry: serde_json::Value =
        serde_json::from_str(&deployment.cli(&["registry", "pull"])).unwrap();
    let route = &mut registry["service_directory"]["services"][SERVICE];
    route["active_host"] = json!(SILENT);
    route["endpoints"][SILENT] = json!({ "url": format!("http://{service_bind}") });
    let file = deployment.root.join("silent.json");
    std::fs::write(&file, serde_json::to_vec_pretty(&registry).unwrap()).unwrap();
    deployment.cli(&["registry", "import", &file.to_string_lossy()]);
    let log = deployment.serve(input("STADO_TEST_FD_LIMIT"));

    let (first, first_took) = ask(&adapter_bind);
    assert!(
        first.starts_with("HTTP/1.1 502"),
        "the first client is refused: {first}"
    );
    assert!(
        first.contains("no registry SSH connection path answered"),
        "{first}"
    );

    let (second, second_took) = ask(&adapter_bind);
    assert!(
        second.starts_with("HTTP/1.1 502"),
        "the second client is refused: {second}"
    );
    assert!(
        second.contains(&format!(
            "not tried again: every SSH path of {SILENT} refused"
        )) && second.contains(&format!("{refresh}s refresh interval")),
        "the second refusal names the remembered refusal and the interval: {second}"
    );
    assert!(
        second_took <= first_took,
        "the remembered refusal took {second_took:?}, the attempt {first_took:?}"
    );

    std::thread::sleep(Duration::from_secs(refresh));
    let (third, _) = ask(&adapter_bind);
    assert!(
        third.contains("no registry SSH connection path answered")
            && !third.contains("not tried again"),
        "after the interval the host is tried again: {third}"
    );
    drop(deployment);
    let said = log.join().expect("the resolver's log is read");
    assert!(
        said.iter().any(|line| line.contains("not tried again")),
        "the resolver logs it"
    );
}
