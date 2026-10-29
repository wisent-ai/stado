//! `stado network` refusals, each driven through the built binary against an
//! isolated local store, with the exact sentence and the registry document
//! left on disk asserted. WC_STORAGE_BACKEND=local and WC_LOCAL_STORAGE_PATH
//! point at the test's own directory, HOME at a directory beside it, and
//! STADO_CONFIG at an empty config, so no operator state is read or written.

use std::path::Path;
use std::process::{Command, Output};

use serde_json::Value;

const REGISTRY: &str = r#"{
    "schema_version": 2,
    "targets": [
        {
            "name": "example-host",
            "kind": "local",
            "ssh": "operator@example-host.invalid",
            "release_platform": "linux-amd64",
            "hostnames": ["example-host.invalid"]
        },
        {
            "name": "example-peer",
            "kind": "local",
            "ssh": "operator@example-peer.stado.internal",
            "release_platform": "linux-amd64",
            "hostnames": ["example-peer.invalid"],
            "network": {"public_key": "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA="}
        }
    ],
    "coordinators": []
}"#;

struct Fixture {
    root: std::path::PathBuf,
}

impl Fixture {
    fn new(name: &str) -> Self {
        let root = Path::new(env!("CARGO_TARGET_TMPDIR"))
            .join(format!("network-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("store")).expect("create the store");
        std::fs::create_dir_all(root.join("home")).expect("create the home");
        std::fs::write(root.join("store/registry.json"), REGISTRY).expect("seed the registry");
        std::fs::write(root.join("config.json"), "{}").expect("write the empty config");
        Self { root }
    }

    fn stado(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_stado"))
            .args(args)
            .env("WC_STORAGE_BACKEND", "local")
            .env("WC_LOCAL_STORAGE_PATH", self.root.join("store"))
            .env("STADO_CONFIG", self.root.join("config.json"))
            .env("HOME", self.root.join("home"))
            .output()
            .expect("the stado binary runs")
    }

    fn registry(&self) -> String {
        std::fs::read_to_string(self.root.join("store/registry.json"))
            .expect("the registry stays on disk")
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn network_up_refuses_bad_input_before_changing_anything() {
    let fixture = Fixture::new("up");

    let output = fixture.stado(&[
        "network",
        "up",
        "--host",
        "example-host",
        "--endpoint",
        "no-port",
    ]);
    assert_eq!(output.status.code(), Some(2), "{}", stderr(&output));
    assert!(
        stderr(&output).contains(r#"--endpoint "no-port" is not HOST:PORT with a nonzero port"#),
        "{}",
        stderr(&output)
    );

    let output = fixture.stado(&["network", "up", "--host", "example-host", "--relay"]);
    assert_eq!(output.status.code(), Some(2), "{}", stderr(&output));
    assert!(
        stderr(&output).contains("--attach-retry-seconds"),
        "{}",
        stderr(&output)
    );

    let output = fixture.stado(&[
        "network",
        "up",
        "--host",
        "example-host",
        "--relay",
        "--attach-retry-seconds",
        "0",
    ]);
    assert_eq!(output.status.code(), Some(2), "{}", stderr(&output));
    assert!(
        stderr(&output).contains("--attach-retry-seconds must be at least 1"),
        "{}",
        stderr(&output)
    );

    let output = fixture.stado(&["network", "up", "--host", "no-such-host"]);
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("the registry declares no host no-such-host"),
        "{}",
        stderr(&output)
    );

    assert_eq!(
        fixture.registry(),
        REGISTRY,
        "a refused up leaves the registry as it was"
    );
    assert!(
        !fixture.root.join("home/.stado/network/key").exists(),
        "a refused up mints no key"
    );
}

#[test]
fn network_status_refuses_a_host_that_has_not_joined() {
    let fixture = Fixture::new("status");
    let output = fixture.stado(&["network", "status", "--json"]);
    assert!(!output.status.success());
    assert!(
        stderr(&output)
            .contains("this host has not joined the Stado network; `stado network up` joins it"),
        "{}",
        stderr(&output)
    );
    let report: Value =
        serde_json::from_slice(&output.stdout).expect("status prints its report before refusing");
    assert_eq!(report["serving"], Value::Bool(false));
    assert_eq!(
        report["not_on_network"],
        serde_json::json!(["example-host"])
    );
}

#[test]
fn network_down_refuses_to_leave_a_host_without_a_path() {
    let fixture = Fixture::new("down");
    let output = fixture.stado(&["network", "down", "--host", "example-peer"]);
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains(
            "registry host example-peer has no SSH path but the Stado network, so taking the network down would leave it unreachable; `stado registry host path set example-peer lan --ssh USER@HOST` declares another first"
        ),
        "{}",
        stderr(&output)
    );
    assert_eq!(
        fixture.registry(),
        REGISTRY,
        "a refused down leaves the registry as it was"
    );
}

#[test]
fn network_connect_refuses_without_this_hosts_key() {
    let fixture = Fixture::new("connect");
    let output = fixture.stado(&["network", "connect", "example-peer.stado.internal", "22"]);
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("this account has no Stado network key at")
            && stderr(&output).contains("`stado network up` creates it"),
        "{}",
        stderr(&output)
    );
}
