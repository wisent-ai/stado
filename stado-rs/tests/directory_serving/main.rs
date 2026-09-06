//! What `stado registry doctor` must say about a directory that sends
//! consumers somewhere a rollout will move, and about a document every
//! resolver refuses.
//!
//! Both defects took product chat down on 2026-09-06 and neither was reported
//! by anything:
//!
//! * The service directory named brama's candidate port `127.0.0.1:18080`
//!   while `release_control` declared the stable bind `127.0.0.1:8080`. Three
//!   rollouts in one evening each moved the rollout to the other candidate
//!   port, and every consumer lost the gateway the moment it moved.
//! * One route alias without a namespace was published into
//!   `inference.routes`. Every resolver validates the document before adopting
//!   a generation, so each refused it, kept serving the generation it had last
//!   accepted, and handed consumers an address the registry no longer
//!   declared. The always-on host's resolver sat eleven generations behind and
//!   the only trace was one line in its own log.
//!
//! Every test drives the built `stado` binary (`CARGO_BIN_EXE_stado`) against
//! a temporary local store with `WC_STORAGE_BACKEND=local`, a temporary HOME
//! so the operator's last-known-good copy is never written, and a
//! `STADO_CONFIG` pointing at a nonexistent file so the developer's real
//! configuration cannot leak in. The documents are the live `brama` blocks
//! from `charless-mac-mini`, trimmed to the keys these findings read.

use std::path::PathBuf;
use std::process::{Command, Output};

/// The fleet document, with the brama endpoint and the inference routes left
/// as placeholders each test fills in. `release_control` is the live
/// blue-green policy: stable bind 8080, candidate ports 18080 and 18081.
fn registry(endpoint: &str, routes: &str) -> String {
    format!(
        r#"{{
    "schema_version": 2,
    "coordinators": [],
    "targets": [
        {{
            "name": "charless-mac-mini",
            "kind": "local",
            "ssh": "charles@100.120.25.24",
            "release_platform": "darwin-arm64",
            "hostnames": ["charless-mac-mini.local"],
            "services": [
                {{
                    "kind": "launchd",
                    "name": "com.wisent.always-on.brama",
                    "label": "com.wisent.always-on.brama",
                    "path": "/Library/LaunchDaemons/com.wisent.always-on.brama.plist",
                    "unit": ""
                }}
            ]
        }}
    ],
    "service_directory": {{
        "authority": {{
            "target": "charless-mac-mini",
            "command": "/Users/charles/.stado/bin/stado"
        }},
        "generation": 47,
        "services": {{
            "brama": {{
                "active_host": "charless-mac-mini",
                "managed_service": "com.wisent.always-on.brama",
                "consumers": {{
                    "wisent-backend": {{ "capabilities": ["model-routing"] }}
                }},
                "endpoints": {{
                    "charless-mac-mini": {{ "url": "{endpoint}" }}
                }}
            }}
        }}
    }},
    "release_control": {{
        "schema_version": 1,
        "generation": 57,
        "trusted_keys": {{}},
        "products": {{
            "brama": {{
                "service": "brama",
                "config_schema": 1,
                "state_schema": 1,
                "install_root": "/Users/charles/.stado/services/brama",
                "binary": "bin/brama",
                "launcher": "bin/start-with-skarbiec",
                "binary_env": "BRAMA_BIN",
                "port_env": "BRAMA_PORT_OVERRIDE",
                "runtime_env": "BRAMA_RUNTIME_DIR",
                "signing_key_id": "",
                "signing_key_item": "",
                "strategy": {{
                    "kind": "blue-green",
                    "readiness_timeout_seconds": 90,
                    "drain_timeout_seconds": 60,
                    "rollback_window_seconds": 300,
                    "automatic_rollback": true
                }},
                "targets": {{
                    "charless-mac-mini": {{
                        "platform": "darwin-arm64",
                        "run_as_user": "charles",
                        "home": "/Users/charles",
                        "state_dir": "/Users/charles/.stado/release-state",
                        "runtime_root": "/Users/charles/.stado/run",
                        "logs_root": "/Users/charles/.stado/logs",
                        "stable_bind": "127.0.0.1:8080",
                        "candidate_ports": [18080, 18081],
                        "readiness_path": "/readyz"
                    }}
                }}
            }}
        }}
    }},
    "inference": {routes}
}}"#
    )
}

/// The routes block as the fleet carries it: a namespaced alias and a
/// single-segment one, both resolving to the running local deployment.
const HEALTHY_ROUTES: &str = r#"{
        "gateway_target": "charless-mac-mini",
        "deployments": [],
        "routes": {},
        "fallbacks": {}
    }"#;

/// The alias that froze every resolver: a name with an empty second segment,
/// which no consumer can route on.
const REFUSED_ROUTES: &str = r#"{
        "gateway_target": "charless-mac-mini",
        "deployments": [],
        "routes": { "wisent-backend/": "best" },
        "fallbacks": {}
    }"#;

struct Fleet {
    home: tempfile::TempDir,
    storage: tempfile::TempDir,
}

impl Fleet {
    fn new(document: &str) -> Self {
        let fleet = Self {
            home: tempfile::tempdir().unwrap(),
            storage: tempfile::tempdir().unwrap(),
        };
        std::fs::write(fleet.registry_blob(), document).unwrap();
        fleet
    }

    fn registry_blob(&self) -> PathBuf {
        self.storage.path().join("registry.json")
    }

    fn stado(&self, args: &[&str]) -> Output {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_stado"));
        cmd.args(args)
            .env("HOME", self.home.path())
            .env("WC_STORAGE_BACKEND", "local")
            .env("WC_LOCAL_STORAGE_PATH", self.storage.path())
            .env(
                "STADO_CONFIG",
                self.storage.path().join("no-such-config.json"),
            )
            .env_remove("COMPUTE_API_KEY")
            .env_remove("COMPUTE_API_URL")
            .env_remove("WC_PROFILES_DIR");
        cmd.output().expect("stado binary runs")
    }

    /// Every finding kind `registry doctor --json` reported.
    fn doctor_kinds(&self) -> Vec<String> {
        let out = self.stado(&["registry", "doctor", "--json"]);
        let body = String::from_utf8_lossy(&out.stdout).into_owned();
        let start = body
            .find('{')
            .unwrap_or_else(|| panic!("doctor printed no JSON: {body}"));
        let report: serde_json::Value = serde_json::from_str(&body[start..])
            .unwrap_or_else(|error| panic!("doctor JSON did not parse ({error}): {body}"));
        report["findings"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .filter_map(|finding| {
                finding
                    .get("finding")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string)
            })
            .collect()
    }

    fn write(&self, path: &str, document: &str) -> PathBuf {
        let file = self.storage.path().join(path);
        std::fs::write(&file, document).unwrap();
        file
    }
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

#[test]
fn doctor_reports_a_directory_that_names_a_candidate_port() {
    let fleet = Fleet::new(&registry("http://127.0.0.1:18080", HEALTHY_ROUTES));

    let kinds = fleet.doctor_kinds();

    assert!(
        kinds.iter().any(|kind| kind == "directory-names-candidate-port"),
        "a directory sending consumers to a rollout's candidate port must be reported, got {kinds:?}"
    );
}

#[test]
fn doctor_accepts_a_directory_that_names_the_stable_bind() {
    let fleet = Fleet::new(&registry("http://127.0.0.1:8080", HEALTHY_ROUTES));

    let kinds = fleet.doctor_kinds();

    assert!(
        !kinds
            .iter()
            .any(|kind| kind == "directory-names-candidate-port"),
        "the declared stable bind is the address consumers must be given, got {kinds:?}"
    );
}

#[test]
fn doctor_reports_a_document_every_resolver_refuses() {
    let fleet = Fleet::new(&registry("http://127.0.0.1:8080", REFUSED_ROUTES));

    let kinds = fleet.doctor_kinds();

    assert!(
        kinds.iter().any(|kind| kind == "resolver-refuses-registry"),
        "a published document the inference contract refuses freezes every resolver and must be reported, got {kinds:?}"
    );
}

#[test]
fn doctor_is_silent_when_the_document_is_one_resolvers_accept() {
    let fleet = Fleet::new(&registry("http://127.0.0.1:8080", HEALTHY_ROUTES));

    let kinds = fleet.doctor_kinds();

    assert!(
        !kinds.iter().any(|kind| kind == "resolver-refuses-registry"),
        "a document the contract accepts must not be reported as refused, got {kinds:?}"
    );
}

#[test]
fn validate_refuses_a_route_alias_with_an_empty_segment() {
    let fleet = Fleet::new(&registry("http://127.0.0.1:8080", HEALTHY_ROUTES));
    let candidate = fleet.write(
        "candidate.json",
        &registry("http://127.0.0.1:8080", REFUSED_ROUTES),
    );

    let out = fleet.stado(&["registry", "validate", candidate.to_str().unwrap()]);

    assert!(
        !out.status.success(),
        "an unroutable alias must be refused before publication"
    );
    assert!(
        stderr(&out).contains("wisent-backend/"),
        "the refusal must name the alias it refuses, got: {}",
        stderr(&out)
    );
}

#[test]
fn validate_accepts_a_single_segment_route_alias() {
    let single = r#"{
        "gateway_target": "charless-mac-mini",
        "deployments": [],
        "routes": { "wisent-backend": "best" },
        "fallbacks": {}
    }"#;
    let fleet = Fleet::new(&registry("http://127.0.0.1:8080", HEALTHY_ROUTES));
    let candidate = fleet.write("candidate.json", &registry("http://127.0.0.1:8080", single));

    let out = fleet.stado(&["registry", "validate", candidate.to_str().unwrap()]);

    assert!(
        out.status.success(),
        "brama's own chat alias carries no second segment and must be publishable: {}",
        stderr(&out)
    );
}
