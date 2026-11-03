//! One host, one owner per loopback port, through the real `stado registry
//! validate`.
//!
//! A host's resolver adapter bound to a loopback port that a release of the
//! same host declares as its stable bind killed the release on every rollout
//! (its candidate died on `Address already in use`) while both declarations
//! read correct on their own. The journey writes the shared template registry
//! with a blue-green release on `marker-host`, then:
//! - the resolver's adapter on the release's stable bind is refused, naming the
//!   adapter, the port, the release and that one port has one owner;
//! - the adapter on one of the release's candidate ports is refused the same way;
//! - the adapter on a port the release does not declare validates.
//!
//! Run with `cargo test --manifest-path stado-rs/Cargo.toml --test
//! service-directory-port-owner`.
use serde_json::{json, Value};
use std::fs;
use std::path::PathBuf;
use std::process::{Command, Stdio};

const TARGET: &str = "marker-host";
const SERVICE: &str = "marker-api";
const STABLE: u16 = 18_201;
const CANDIDATES: [u16; 2] = [18_202, 18_203];

/// The template registry with `marker-host`'s resolver serving one adapter
/// on `adapter_port`, and a blue-green release of `marker-api` on that host.
fn registry(adapter_port: u16) -> Value {
    let template = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../tests/service_directory/registry.json");
    let mut document: Value = serde_json::from_slice(&fs::read(template).unwrap()).unwrap();
    let target = document["targets"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|target| target["name"] == TARGET)
        .unwrap();
    target["service_resolver"]["adapters"] = json!([{
        "service": SERVICE,
        "consumer": "marker-consumer",
        "bind": format!("127.0.0.1:{adapter_port}"),
    }]);
    document["release_control"] = json!({
        "schema_version": 1,
        "generation": 1,
        "trusted_keys": {},
        "products": {
            SERVICE: {
                "service": SERVICE,
                "config_schema": 1,
                "state_schema": 1,
                "install_root": "/Users/marker/.stado/services/marker-api",
                "binary": "bin/marker-api",
                "launcher": "bin/start",
                "binary_env": "MARKER_API_BIN",
                "port_env": "MARKER_API_PORT",
                "runtime_env": "MARKER_API_RUNTIME_DIR",
                "strategy": {
                    "kind": "blue-green",
                    "rollback_window_seconds": 300,
                    "automatic_rollback": true,
                    "drain_timeout_seconds": 60,
                    "readiness_timeout_seconds": 90
                },
                "targets": {
                    TARGET: {
                        "platform": "darwin-arm64",
                        "run_as_user": "marker",
                        "home": "/Users/marker",
                        "state_dir": "/Users/marker/.stado/release-state",
                        "runtime_root": "/Users/marker/.stado/run",
                        "logs_root": "/Users/marker/.stado/logs",
                        "stable_bind": format!("127.0.0.1:{STABLE}"),
                        "candidate_ports": CANDIDATES,
                        "readiness_path": "/healthz"
                    }
                }
            }
        }
    });
    document
}

struct Journey {
    root: PathBuf,
    report: Value,
}

impl Journey {
    fn start() -> Self {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../.build/service-directory-port-owner")
            .join(uuid::Uuid::new_v4().to_string());
        fs::create_dir_all(root.join("tmp")).unwrap();
        Self {
            root,
            report: json!({
                "source_revision": std::env::var("STADO_SOURCE_REVISION").unwrap_or_default(),
                "cases": [],
                "outcome": "failed",
            }),
        }
    }

    /// `stado registry validate` over `document`, in a home of the journey's
    /// own: whether it passed, and its standard error.
    fn validate(&mut self, case: &str, document: &Value) -> (bool, String) {
        let path = self.root.join(format!("{case}.json"));
        fs::write(&path, serde_json::to_vec_pretty(document).unwrap()).unwrap();
        let output = Command::new(env!("CARGO_BIN_EXE_stado"))
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap())
            .env("HOME", &self.root)
            .env(
                "STADO_CONFIG",
                concat!(env!("CARGO_MANIFEST_DIR"), "/../tests/config/empty.json"),
            )
            .env("TMPDIR", self.root.join("tmp"))
            .args(["registry", "validate"])
            .arg(&path)
            .stdin(Stdio::null())
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        self.report["cases"].as_array_mut().unwrap().push(json!({
            "case": case,
            "registry": path,
            "exit_status": output.status.code(),
            "stdout": String::from_utf8_lossy(&output.stdout),
            "stderr": stderr,
        }));
        self.save();
        (output.status.success(), stderr)
    }

    fn save(&self) {
        fs::write(
            self.root.join("report.json"),
            serde_json::to_vec_pretty(&self.report).unwrap(),
        )
        .unwrap();
    }
}

#[test]
fn a_loopback_port_a_release_declares_is_refused_to_a_resolver_adapter() {
    let mut journey = Journey::start();

    let (passed, stderr) = journey.validate("apart", &registry(18_210));
    assert!(passed, "an adapter on a port no release declares validates: {stderr}");

    for (case, port, what) in [
        ("on-stable-bind", STABLE, "stable_bind"),
        ("on-candidate-port", CANDIDATES[1], "candidate port"),
    ] {
        let (passed, stderr) = journey.validate(case, &registry(port));
        assert!(!passed, "{case}: an adapter on the release's {what} is refused");
        assert!(
            stderr.contains(&format!(
                "registry.targets[{TARGET}].service_resolver: resolver adapter {SERVICE} claims \
                 loopback port {port}, which registry.release_control.products.{SERVICE} \
                 declares as its {what} on this host; one loopback port has one owner"
            )),
            "{case}: the refusal names the adapter, the port and the release: {stderr}"
        );
    }

    journey.report["outcome"] = json!("passed");
    journey.save();
}
