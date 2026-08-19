//! Fleet lifecycle tests against the local storage backend.
//!
//! Every test drives the built `stado` binary (`CARGO_BIN_EXE_stado`) with
//! WC_STORAGE_BACKEND=local + WC_LOCAL_STORAGE_PATH=<TempDir>. STADO_CONFIG
//! points at a nonexistent path so the developer's real config can never
//! leak into a test. The registry document lives and dies inside the temp
//! dir; nothing here touches the operator's real registry or vault.

use std::path::Path;
use std::process::{Command, Output};

fn stado(storage: &Path, args: &[&str]) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_stado"));
    cmd.args(args)
        .env("WC_STORAGE_BACKEND", "local")
        .env("WC_LOCAL_STORAGE_PATH", storage)
        // A set-but-missing STADO_CONFIG disables config-file discovery.
        .env("STADO_CONFIG", storage.join("no-such-config.json"))
        .env_remove("COMPUTE_API_KEY")
        .env_remove("COMPUTE_API_URL")
        .env_remove("WC_PROFILES_DIR");
    cmd.output().expect("stado binary runs")
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

/// A temp storage root carrying exactly this registry document.
fn storage_with_registry(document: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("registry.json"), document).unwrap();
    dir
}

/// The registry document on disk, parsed.
fn registry(storage: &Path) -> serde_json::Value {
    let body = std::fs::read_to_string(storage.join("registry.json")).unwrap();
    serde_json::from_str(&body).expect("registry.json stays JSON")
}

const EMPTY_REGISTRY: &str = r#"{"schema_version": 2, "targets": [], "coordinators": []}"#;

#[test]
fn fleet_create_writes_the_fleet_into_the_registry() {
    let dir = storage_with_registry(EMPTY_REGISTRY);
    let storage = dir.path();

    let out = stado(storage, &["fleet", "create", "build", "--notes", "ci builders"]);
    assert!(out.status.success(), "create failed: {}", stderr(&out));
    assert!(
        stdout(&out).contains("fleet 'build' created"),
        "create reports the fleet in its own sentence, got: {}",
        stdout(&out)
    );

    let fleets = registry(storage)["fleets"].clone();
    assert_eq!(
        fleets,
        serde_json::json!([{"name": "build", "notes": "ci builders"}]),
        "the document on disk is the contract, not the stdout"
    );

    // Creating it again refuses instead of silently overwriting the notes.
    let out = stado(storage, &["fleet", "create", "build"]);
    assert!(
        !out.status.success(),
        "a duplicate create succeeded: {}",
        stdout(&out)
    );
    assert!(
        stderr(&out).contains("fleet 'build' already exists"),
        "the refusal names the fleet, got: {}",
        stderr(&out)
    );
    assert_eq!(
        registry(storage)["fleets"][0]["notes"], "ci builders",
        "the refused create left the existing fleet untouched"
    );

    // A name that is not a lowercase identifier never reaches the document.
    let out = stado(storage, &["fleet", "create", "BAD NAME"]);
    assert!(!out.status.success());
    assert!(
        stderr(&out)
            .contains("registry.fleets[1].name: must be a lowercase fleet identifier"),
        "the refusal names the document location, got: {}",
        stderr(&out)
    );
    assert_eq!(
        registry(storage)["fleets"].as_array().unwrap().len(),
        1,
        "the malformed create wrote nothing"
    );
}
