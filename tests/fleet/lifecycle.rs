//! Fleet lifecycle: create a fleet, edit it by assigning a machine into it,
//! and delete it, each through the built `stado` binary, checking the
//! registry document the command wrote and every refusal by its words.
//!
//! Every test drives `CARGO_BIN_EXE_stado` with WC_STORAGE_BACKEND=local and
//! WC_LOCAL_STORAGE_PATH set to its own temporary directory. STADO_CONFIG
//! points at a path that does not exist, so the developer's real config and
//! registry are never read or written.

use std::path::Path;
use std::process::{Command, Output};

use serde_json::{json, Value};

fn stado(storage: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_stado"))
        .args(args)
        .env("WC_STORAGE_BACKEND", "local")
        .env("WC_LOCAL_STORAGE_PATH", storage)
        // A set-but-missing STADO_CONFIG disables config-file discovery.
        .env("STADO_CONFIG", storage.join("no-such-config.json"))
        .env_remove("COMPUTE_API_KEY")
        .env_remove("COMPUTE_API_URL")
        .env_remove("WC_PROFILES_DIR")
        .output()
        .expect("the stado binary runs")
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

/// A temporary storage root holding exactly this registry document.
fn storage_with_registry(document: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("a temporary storage root");
    std::fs::write(dir.path().join("registry.json"), document)
        .expect("the registry document is written");
    dir
}

/// The registry document on disk, parsed.
fn registry(storage: &Path) -> Value {
    let body = std::fs::read_to_string(storage.join("registry.json"))
        .expect("the registry document stays on disk");
    serde_json::from_str(&body).expect("registry.json stays JSON")
}

/// The fleet the registry's one machine, `w1`, belongs to.
fn fleet_of_w1(document: &Value) -> Value {
    document["targets"]
        .as_array()
        .and_then(|targets| targets.iter().find(|target| target["name"] == "w1"))
        .map(|target| target["fleet"].clone())
        .expect("the registry still holds w1")
}

const EMPTY_REGISTRY: &str = r#"{"schema_version": 2, "targets": [], "coordinators": []}"#;

const REGISTRY_WITH_FLEET: &str = r#"{
    "schema_version": 2,
    "targets": [
        {
            "name": "w1",
            "kind": "local",
            "ssh": "w1-host.invalid",
            "release_platform": "linux-amd64",
            "hostnames": ["w1-host.invalid"]
        }
    ],
    "coordinators": [],
    "fleets": [{"name": "build", "notes": "ci"}]
}"#;

const REGISTRY_WITH_MEMBER: &str = r#"{
    "schema_version": 2,
    "targets": [
        {
            "name": "w1",
            "kind": "local",
            "ssh": "w1-host.invalid",
            "release_platform": "linux-amd64",
            "hostnames": ["w1-host.invalid"],
            "fleet": "build"
        }
    ],
    "coordinators": [],
    "fleets": [{"name": "build", "notes": "ci"}, {"name": "edge"}]
}"#;

#[test]
fn fleet_create_writes_the_fleet_into_the_registry() {
    let dir = storage_with_registry(EMPTY_REGISTRY);
    let storage = dir.path();

    let out = stado(
        storage,
        &["fleet", "create", "build", "--notes", "ci builders"],
    );
    assert!(out.status.success(), "create failed: {}", stderr(&out));
    assert!(
        stdout(&out).contains("fleet 'build' created"),
        "got: {}",
        stdout(&out)
    );
    assert_eq!(
        registry(storage)["fleets"],
        json!([{"name": "build", "notes": "ci builders"}]),
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
        "got: {}",
        stderr(&out)
    );

    // A name that is not a lowercase identifier never reaches the document.
    let out = stado(storage, &["fleet", "create", "BAD NAME"]);
    assert!(
        !out.status.success(),
        "a malformed name was created: {}",
        stdout(&out)
    );
    assert!(
        stderr(&out).contains("must be a lowercase fleet identifier"),
        "got: {}",
        stderr(&out)
    );
    assert_eq!(
        registry(storage)["fleets"],
        json!([{"name": "build", "notes": "ci builders"}]),
        "the refused creates left the document as the first create wrote it"
    );
}

#[test]
fn fleet_assign_edits_the_fleet_by_moving_a_target_into_it() {
    let dir = storage_with_registry(REGISTRY_WITH_FLEET);
    let storage = dir.path();

    let out = stado(storage, &["fleet", "assign", "w1", "build"]);
    assert!(out.status.success(), "assign failed: {}", stderr(&out));
    assert!(
        stdout(&out).contains("target 'w1' assigned to fleet 'build'"),
        "got: {}",
        stdout(&out)
    );
    assert_eq!(fleet_of_w1(&registry(storage)), "build");

    // A machine the registry does not hold is refused by name.
    let out = stado(storage, &["fleet", "assign", "ghost", "build"]);
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("target 'ghost' not found in registry"),
        "got: {}",
        stderr(&out)
    );

    // A fleet that was never declared is refused by name.
    let out = stado(storage, &["fleet", "assign", "w1", "ghost"]);
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("fleet 'ghost' is not declared; create it first"),
        "got: {}",
        stderr(&out)
    );

    let document = registry(storage);
    assert_eq!(fleet_of_w1(&document), "build");
    assert_eq!(
        document["fleets"],
        json!([{"name": "build", "notes": "ci"}])
    );
}

#[test]
fn fleet_delete_retires_only_an_unmanned_fleet() {
    let dir = storage_with_registry(REGISTRY_WITH_MEMBER);
    let storage = dir.path();

    // The empty fleet goes, and the document on disk proves it.
    let out = stado(storage, &["fleet", "delete", "edge"]);
    assert!(out.status.success(), "delete failed: {}", stderr(&out));
    assert!(
        stdout(&out).contains("fleet 'edge' deleted"),
        "got: {}",
        stdout(&out)
    );
    assert_eq!(
        registry(storage)["fleets"],
        json!([{"name": "build", "notes": "ci"}])
    );

    // Deleting it again is a refusal, not a second success.
    let out = stado(storage, &["fleet", "delete", "edge"]);
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("fleet 'edge' is not declared"),
        "got: {}",
        stderr(&out)
    );

    // A fleet with a member is not deleted out from under it: the refusal
    // names the member and how to move it, and the document keeps both.
    let out = stado(storage, &["fleet", "delete", "build"]);
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("fleet 'build' still has 1 member(s): w1"),
        "got: {}",
        stderr(&out)
    );
    assert!(
        stderr(&out).contains("stado fleet unassign TARGET"),
        "got: {}",
        stderr(&out)
    );
    let document = registry(storage);
    assert_eq!(
        document["fleets"],
        json!([{"name": "build", "notes": "ci"}])
    );
    assert_eq!(fleet_of_w1(&document), "build");
}
