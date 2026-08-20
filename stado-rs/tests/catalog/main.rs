//! `stado service catalog` and the catalog resolution seam, against the
//! local storage backend. The catalog is compiled into the binary, so the
//! listing needs no store at all; the resolution failure path is driven
//! through `service ensure` on a seeded registry naming this machine.

use std::path::Path;
use std::process::{Command, Output};

fn stado(storage: &Path, args: &[&str]) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_stado"));
    cmd.args(args)
        .env("WC_STORAGE_BACKEND", "local")
        .env("WC_LOCAL_STORAGE_PATH", storage)
        .env("STADO_CONFIG", storage.join("no-such-config.json"))
        .env_remove("COMPUTE_API_KEY")
        .env_remove("COMPUTE_API_URL");
    cmd.output().expect("stado binary runs")
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

#[test]
fn the_catalog_lists_the_wisent_services() {
    let dir = tempfile::tempdir().unwrap();
    let out = stado(dir.path(), &["service", "catalog"]);
    assert!(out.status.success(), "catalog failed: {}", stderr(&out));
    let text = stdout(&out);
    for name in ["skarbiec", "brama", "weles", "stado-agent"] {
        assert!(text.contains(name), "missing {name} in: {text}");
    }
}

#[test]
fn the_json_catalog_carries_program_and_args() {
    let dir = tempfile::tempdir().unwrap();
    let out = stado(dir.path(), &["service", "catalog", "--json"]);
    assert!(out.status.success(), "catalog failed: {}", stderr(&out));
    let document: serde_json::Value =
        serde_json::from_str(&stdout(&out)).expect("catalog --json emits JSON");
    let services = document["services"].as_array().expect("services array");
    assert_eq!(services.len(), 4);
    let skarbiec = services
        .iter()
        .find(|entry| entry["name"] == "skarbiec")
        .expect("skarbiec entry");
    assert_eq!(skarbiec["program"], "$HOME/.stado/bin/skarbiec");
    assert_eq!(skarbiec["args"], serde_json::json!(["serve", "--port", "8895"]));
    let agent = services
        .iter()
        .find(|entry| entry["name"] == "stado-agent")
        .expect("agent entry");
    assert_eq!(agent["args"], serde_json::json!(["agent", "--auto"]));
}

#[test]
fn a_name_outside_the_catalog_names_the_catalog_in_its_refusal() {
    let dir = tempfile::tempdir().unwrap();
    let hostname = String::from_utf8(Command::new("hostname").output().unwrap().stdout)
        .unwrap()
        .trim()
        .split('.')
        .next()
        .unwrap()
        .to_string();
    let document = format!(
        r#"{{"schema_version": 2, "targets": [{{
            "name": "this-mac", "kind": "local", "ssh": null,
            "release_platform": "darwin-arm64",
            "hostnames": ["{hostname}"], "slots": 1
        }}], "coordinators": []}}"#
    );
    std::fs::write(dir.path().join("registry.json"), document).unwrap();
    let out = stado(
        dir.path(),
        &[
            "service", "ensure", "no-such-thing",
            "--host", "this-mac",
            "--reason", "test",
        ],
    );
    assert!(!out.status.success(), "an undeclared name must refuse");
    assert!(
        stderr(&out).contains("stado service catalog"),
        "the refusal points at the catalog: {}",
        stderr(&out)
    );
}
