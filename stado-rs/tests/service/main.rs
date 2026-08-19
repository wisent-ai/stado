//! `stado service declare` contract tests against the local storage backend.
//!
//! Every test drives the built `stado` binary (`CARGO_BIN_EXE_stado`) with
//! WC_STORAGE_BACKEND=local + WC_LOCAL_STORAGE_PATH=<TempDir> and a
//! STADO_CONFIG pointing at a nonexistent path, so the developer's real
//! config can never leak into a test. HOME is a tempdir too, so the
//! registry last-known-good cache can never reach the operator's real one.
//!
//! What is defended here: a declaration the contract accepts lands in the
//! directory with its source, run spec, endpoints and consumers intact, and
//! every refusal — bad name, unknown host, non-immutable digest, missing
//! consumers, unknown verify kind — refuses with its exact sentence and
//! leaves the document untouched.

use std::path::PathBuf;
use std::process::{Command, Output};

/// A valid registry-v2 document: one host, plus a service directory whose
/// authority that host publishes. `services` starts empty; the first declare
/// fills it.
const SEEDED_REGISTRY: &str = r#"{
    "schema_version": 2,
    "targets": [
        {
            "name": "w1",
            "kind": "local",
            "ssh": "u@10.0.0.1",
            "release_platform": "linux-amd64",
            "hostnames": ["w1.local"],
            "slots": 1
        }
    ],
    "coordinators": [],
    "service_directory": {
        "authority": {"target": "w1", "command": "/usr/local/bin/stado"},
        "generation": 1,
        "services": {}
    }
}"#;

const DIGEST: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

/// A HOME and a storage root, both temporary, plus the seeded document.
struct Fleet {
    home: tempfile::TempDir,
    storage: tempfile::TempDir,
}

impl Fleet {
    fn new() -> Self {
        let fleet = Self {
            home: tempfile::tempdir().unwrap(),
            storage: tempfile::tempdir().unwrap(),
        };
        std::fs::write(fleet.registry_blob(), SEEDED_REGISTRY).unwrap();
        fleet
    }

    fn registry_blob(&self) -> PathBuf {
        self.storage.path().join("registry.json")
    }

    fn document(&self) -> serde_json::Value {
        let body = std::fs::read_to_string(self.registry_blob()).expect("registry blob exists");
        serde_json::from_str(&body).expect("registry stays JSON")
    }

    /// Write one declaration file and return its path.
    fn declaration_file(&self, body: &str) -> PathBuf {
        let path = self.storage.path().join("declaration.json");
        std::fs::write(&path, body).unwrap();
        path
    }

    fn stado(&self, args: &[&str]) -> Output {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_stado"));
        cmd.args(args)
            .env("HOME", self.home.path())
            .env("WC_STORAGE_BACKEND", "local")
            .env("WC_LOCAL_STORAGE_PATH", self.storage.path())
            .env("STADO_CONFIG", self.storage.path().join("no-such-config.json"))
            .env_remove("COMPUTE_API_KEY")
            .env_remove("COMPUTE_API_URL")
            .env_remove("WC_PROFILES_DIR");
        cmd.output().expect("stado binary runs")
    }

    fn declare(&self, body: &str) -> Output {
        let path = self.declaration_file(body);
        self.stado(&[
            "service",
            "declare",
            "--file",
            path.to_str().expect("declaration path is UTF-8"),
        ])
    }
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn valid_declaration() -> String {
    format!(
        r#"{{
    "name": "example-serving",
    "host": "w1",
    "port": 8080,
    "source": {{"artifact": "stado://releases/example-serving/1.0.0/linux-amd64", "sha256": "{DIGEST}"}},
    "run": {{"args": ["serve"]}},
    "consumers": {{"example-backend": {{"capabilities": ["model-routing"]}}}}
}}"#
    )
}

#[test]
fn declare_writes_the_contract_into_the_directory() {
    let fleet = Fleet::new();

    let out = fleet.declare(&valid_declaration());
    assert!(out.status.success(), "declare failed: {}", stderr(&out));

    let document = fleet.document();
    let entry = &document["service_directory"]["services"]["example-serving"];
    assert_eq!(entry["active_host"], "w1");
    assert_eq!(entry["endpoints"]["w1"]["url"], "http://127.0.0.1:8080");
    assert_eq!(
        entry["declaration"]["source"]["artifact"],
        "stado://releases/example-serving/1.0.0/linux-amd64"
    );
    assert_eq!(entry["declaration"]["source"]["sha256"], DIGEST);
    assert_eq!(entry["declaration"]["run"]["args"][0], "serve");
    assert_eq!(
        entry["consumers"]["example-backend"]["capabilities"][0],
        "model-routing"
    );
}

#[test]
fn declare_refuses_a_digest_that_is_not_immutable() {
    let fleet = Fleet::new();
    let body = valid_declaration().replace(DIGEST, "ABC123");

    let out = fleet.declare(&body);
    assert!(!out.status.success(), "declare accepted a short digest");
    assert!(
        stderr(&out).contains("must be 64 lowercase hex characters"),
        "got: {}",
        stderr(&out)
    );
    // A refused declaration never moves the document.
    assert_eq!(read_document_raw(&fleet), SEEDED_REGISTRY);
}

#[test]
fn declare_refuses_a_host_outside_the_registry() {
    let fleet = Fleet::new();
    let body = valid_declaration().replace("\"host\": \"w1\"", "\"host\": \"w9\"");

    let out = fleet.declare(&body);
    assert!(!out.status.success(), "declare accepted an unknown host");
    assert!(
        stderr(&out).contains("which is not a registry target"),
        "got: {}",
        stderr(&out)
    );
}

#[test]
fn declare_refuses_a_service_without_consumers() {
    let fleet = Fleet::new();
    let body = valid_declaration().replace(
        "    \"run\": {\"args\": [\"serve\"]},\n    \"consumers\": {\"example-backend\": {\"capabilities\": [\"model-routing\"]}}\n",
        "    \"run\": {\"args\": [\"serve\"]}\n",
    );

    let out = fleet.declare(&body);
    assert!(!out.status.success(), "declare accepted no consumers");
    assert!(
        stderr(&out).contains("'consumers' is required"),
        "got: {}",
        stderr(&out)
    );
}

#[test]
fn declare_refuses_an_unknown_verify_kind() {
    let fleet = Fleet::new();
    let body = valid_declaration().replace(
        r#"    "consumers"#,
        r#"    "verify": {"kind": "dns"},
    "consumers"#,
    );

    let out = fleet.declare(&body);
    assert!(!out.status.success(), "declare accepted verify kind dns");
    assert!(
        stderr(&out).contains("unknown value 'dns'"),
        "got: {}",
        stderr(&out)
    );
}

#[test]
fn declare_refuses_a_name_with_empty_edges() {
    let fleet = Fleet::new();
    let body = valid_declaration().replace("example-serving", "-example-");

    let out = fleet.declare(&body);
    assert!(!out.status.success(), "declare accepted a bad name");
    assert!(
        stderr(&out).contains("must be a lowercase identifier without empty edges"),
        "got: {}",
        stderr(&out)
    );
}

#[test]
fn declare_refuses_a_declaration_without_an_endpoint() {
    let fleet = Fleet::new();
    let body = valid_declaration().replace("    \"port\": 8080,\n", "");

    let out = fleet.declare(&body);
    assert!(!out.status.success(), "declare accepted no endpoint");
    assert!(
        stderr(&out).contains("pass 'endpoints' or the 'port' shorthand"),
        "got: {}",
        stderr(&out)
    );
}

fn read_document_raw(fleet: &Fleet) -> String {
    std::fs::read_to_string(fleet.registry_blob()).expect("registry blob exists")
}
