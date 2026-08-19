//! Fleet registry tests against the local storage backend.
//!
//! Every test drives the built `stado` binary (`CARGO_BIN_EXE_stado`) with
//! WC_STORAGE_BACKEND=local + WC_LOCAL_STORAGE_PATH=<TempDir>. STADO_CONFIG
//! points at a nonexistent path so the developer's real config can never
//! leak into a test, and no command here mints anything: invite creation
//! touches the machine's real vault, so this suite covers only the
//! read-and-validate half of the fleet surface.

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

#[test]
fn fleet_list_without_a_registry_names_the_missing_document() {
    let dir = tempfile::tempdir().unwrap();
    let out = stado(dir.path(), &["fleet", "list"]);
    assert!(
        !out.status.success(),
        "fleet list without a registry succeeded: {}",
        stdout(&out)
    );
    assert!(
        stderr(&out).contains("no registry document at local:registry.json"),
        "the failure must name the missing document, got: {}",
        stderr(&out)
    );
}

#[test]
fn fleet_list_reports_members_resolved_from_targets() {
    let dir = storage_with_registry(
        r#"{
            "fleets": [
                {"name": "build", "notes": "ci builders"},
                {"name": "edge"}
            ],
            "targets": [
                {"name": "w1", "fleet": "build"},
                {"name": "w2", "fleet": "build"}
            ],
            "coordinators": []
        }"#,
    );
    let out = stado(dir.path(), &["fleet", "list", "--json"]);
    assert!(out.status.success(), "fleet list failed: {}", stderr(&out));
    let document: serde_json::Value =
        serde_json::from_str(&stdout(&out)).expect("fleet list --json emits JSON");
    let fleets = document["fleets"].as_array().expect("fleets is an array");
    assert_eq!(fleets.len(), 2, "{document}");
    assert_eq!(fleets[0]["name"], "build");
    assert_eq!(fleets[0]["notes"], "ci builders");
    assert_eq!(fleets[0]["members"], serde_json::json!(["w1", "w2"]));
    assert_eq!(fleets[1]["name"], "edge");
    assert_eq!(fleets[1]["members"], serde_json::json!([]));
}

#[test]
fn fleet_list_refuses_a_target_pointing_at_an_undeclared_fleet() {
    let dir = storage_with_registry(
        r#"{"fleets": [], "targets": [{"name": "t1", "fleet": "ghost"}]}"#,
    );
    let out = stado(dir.path(), &["fleet", "list"]);
    assert!(
        !out.status.success(),
        "a dangling fleet reference listed cleanly: {}",
        stdout(&out)
    );
    assert!(
        stderr(&out).contains(
            "registry.targets[0].fleet: target 't1' points at undeclared fleet 'ghost'"
        ),
        "the refusal must name the exact document location, got: {}",
        stderr(&out)
    );
}

#[test]
fn fleet_invites_on_an_empty_store_is_an_empty_list() {
    let dir = storage_with_registry(r#"{"fleets": [], "targets": [], "coordinators": []}"#);
    let out = stado(dir.path(), &["fleet", "invites", "--json"]);
    assert!(out.status.success(), "invites failed: {}", stderr(&out));
    let document: serde_json::Value =
        serde_json::from_str(&stdout(&out)).expect("invites --json emits JSON");
    assert_eq!(document["invites"], serde_json::json!([]));
}
