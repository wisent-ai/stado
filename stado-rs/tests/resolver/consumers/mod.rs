//! Consumer declarations include the resolver route, and removal retires both.
//! These journeys use the real CLI, registry, resolver and Stado HTTP server.
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::fixture::{http_get, listening, wait_listening, Policy, Serving};
use crate::resolution::GENERATION;
use crate::{free_port, held_port, said, Host, CONSUMER, SERVICE, TARGET};

mod refusals;

const CLIENT: &str = "directory-client";

fn stored(host: &Host) -> Value {
    serde_json::from_slice(&fs::read(host.registry_path()).unwrap()).unwrap()
}

fn evidence() -> PathBuf {
    let root = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("consumer-bindings")
        .join(uuid::Uuid::new_v4().to_string());
    fs::create_dir_all(&root).unwrap();
    for (name, args) in [
        ("revision.txt", vec!["rev-parse", "HEAD"]),
        ("changes.patch", vec!["diff", "HEAD"]),
    ] {
        let output = Command::new("git")
            .args(args)
            .current_dir(env!("CARGO_MANIFEST_DIR"))
            .output()
            .unwrap();
        fs::write(root.join(name), &output.stdout).unwrap();
        assert!(output.status.success(), "{}", said(&output));
    }
    let mut binary = fs::File::open(env!("CARGO_BIN_EXE_stado")).unwrap();
    let mut digest = Sha256::new();
    // A bounded I/O buffer, not a copy of the entire executable.
    let mut buffer = [0; 8192];
    loop {
        let count = binary.read(&mut buffer).unwrap();
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
    }
    fs::write(
        root.join("binary.sha256"),
        format!("{:x}\n", digest.finalize()),
    )
    .unwrap();
    println!("Consumer binding evidence: {}", root.display());
    root
}

fn run(host: &Host, evidence: &Path, args: &[&str]) -> Output {
    let output = host.stado(args);
    let receipt = json!({
        "binary": env!("CARGO_BIN_EXE_stado"), "arguments": args,
        "exit_code": output.status.code(),
        "stdout": String::from_utf8_lossy(&output.stdout),
        "stderr": String::from_utf8_lossy(&output.stderr),
        "registry": stored(host),
    });
    fs::write(
        evidence.join(format!("{}.json", uuid::Uuid::new_v4())),
        serde_json::to_vec_pretty(&receipt).unwrap(),
    )
    .unwrap();
    output
}

#[test]
fn declaration_updates_the_consumers_real_route_and_removal_retires_it() {
    let policy = Policy::eager(GENERATION);
    let host = Host::new(&policy.document());
    let evidence = evidence();
    let (original_address, port) = held_port();
    let bind = format!("127.0.0.1:{port}");
    let add = [
        "service",
        "directory",
        "consumer-add",
        SERVICE,
        CLIENT,
        "--target",
        TARGET,
        "--bind",
        &bind,
        "--capability",
        "object-store",
        "--json",
    ];
    let answer = run(&host, &evidence, &add);
    assert!(answer.status.success(), "{}", said(&answer));
    let answer = run(&host, &evidence, &add);
    assert!(answer.status.success(), "{}", said(&answer));
    let document = stored(&host);
    let adapters = document["targets"][0]["service_resolver"]["adapters"]
        .as_array()
        .unwrap();
    assert_eq!(
        adapters.iter().filter(|a| a["consumer"] == CLIENT).count(),
        1
    );

    let replacement = free_port();
    drop(original_address);
    let moved = format!("127.0.0.1:{replacement}");
    let answer = run(
        &host,
        &evidence,
        &[
            "service",
            "directory",
            "consumer-add",
            SERVICE,
            CLIENT,
            "--target",
            TARGET,
            "--bind",
            &moved,
            "--json",
        ],
    );
    assert!(answer.status.success(), "{}", said(&answer));
    assert_eq!(
        stored(&host)["service_directory"]["services"][SERVICE]["consumers"][CLIENT]
            ["capabilities"],
        json!(["object-store"])
    );

    let upstream = Serving::start(
        &host,
        &[
            "dashboard",
            "--bind",
            "127.0.0.1",
            "--port",
            &policy.upstream.to_string(),
        ],
    );
    assert!(wait_listening(policy.upstream), "{}", upstream.said());
    let mut resolver = Serving::start(&host, &["resolver", "serve", "--target", TARGET]);
    assert!(wait_listening(replacement), "{}", resolver.said());
    let resolved = http_get(
        policy.api,
        &format!("/v1/resolve/service/{SERVICE}"),
        &[("X-Stado-Consumer", CLIENT)],
    )
    .unwrap();
    fs::write(evidence.join("resolved.http"), &resolved).unwrap();
    assert!(resolved.starts_with("HTTP/1.1 200"), "{resolved}");
    assert!(resolved.contains(&format!("http://{moved}")), "{resolved}");
    let response = http_get(replacement, "/healthz", &[]).unwrap();
    fs::write(evidence.join("service.http"), &response).unwrap();
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    assert!(response.contains("\"ok\":true"), "{response}");
    assert!(
        !listening(port),
        "the superseded address still has a listener"
    );

    let answer = run(
        &host,
        &evidence,
        &[
            "service",
            "directory",
            "consumer-rm",
            SERVICE,
            CLIENT,
            "--json",
        ],
    );
    assert!(answer.status.success(), "{}", said(&answer));
    let after = stored(&host);
    assert!(after["service_directory"]["services"][SERVICE]["consumers"]
        .get(CLIENT)
        .is_none());
    assert!(after["targets"][0]["service_resolver"]["adapters"]
        .as_array()
        .unwrap()
        .iter()
        .all(|a| a["consumer"] != CLIENT));
    let deadline = Instant::now() + Duration::from_secs(30);
    while resolver.running() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    fs::write(evidence.join("resolver.log"), resolver.said()).unwrap();
    assert!(
        !resolver.running(),
        "the resolver did not observe the removed binding"
    );
    assert!(
        !listening(replacement),
        "the removed consumer still has a listener"
    );
    let answer = run(
        &host,
        &evidence,
        &[
            "resolver",
            "resolve",
            SERVICE,
            "--consumer",
            CLIENT,
            "--json",
        ],
    );
    assert!(
        !answer.status.success(),
        "the removed consumer was still authorized"
    );
    let answer = run(
        &host,
        &evidence,
        &[
            "resolver",
            "resolve",
            SERVICE,
            "--consumer",
            CONSUMER,
            "--json",
        ],
    );
    assert!(
        answer.status.success(),
        "an unrelated consumer was removed: {}",
        said(&answer)
    );
    fs::write(evidence.join("upstream.log"), upstream.said()).unwrap();
}
