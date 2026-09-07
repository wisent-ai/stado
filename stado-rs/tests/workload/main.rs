//! Declaration and refusal tests for the workload command surface.

use std::path::Path;
use std::process::{Command, Output};

fn stado(storage: &Path, args: &[&str]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_stado"));
    command
        .args(args)
        .env("WC_STORAGE_BACKEND", "local")
        .env("WC_LOCAL_STORAGE_PATH", storage)
        .env("STADO_CONFIG", storage.join("no-such-config.json"))
        .env_remove("COMPUTE_API_KEY")
        .env_remove("COMPUTE_API_URL")
        .env_remove("WC_PROFILES_DIR");
    command.output().expect("stado binary runs")
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn storage_with_registry(document: &str) -> tempfile::TempDir {
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(directory.path().join("registry.json"), document).unwrap();
    directory
}

const ALLOWED_REGISTRY: &str = r#"{
  "schema_version": 2,
  "targets": [{
    "name": "worker",
    "kind": "local",
    "ssh": "operator@worker.example",
    "release_platform": "darwin-arm64",
    "hostnames": ["worker.example"],
    "weles": {"enabled": true, "actions": ["generic_capture", "generic_browser_task"]}
  }],
  "coordinators": []
}"#;

const REFUSING_REGISTRY: &str = r#"{
  "schema_version": 2,
  "targets": [{
    "name": "plain",
    "kind": "local",
    "ssh": "operator@plain.example",
    "release_platform": "darwin-arm64",
    "hostnames": ["plain.example"],
    "weles": {"enabled": true, "actions": []}
  }],
  "coordinators": []
}"#;

fn valid_capture_plan(target: &str) -> tempfile::NamedTempFile {
    let file = tempfile::NamedTempFile::new().unwrap();
    let plan = serde_json::json!({
        "schema": "wisent.weles-capture-plan.v1",
        "batch": "test-batch",
        "target": target,
        "captures": [{
            "site_slug": "example",
            "axis": "composition",
            "source_url": "https://example.com/",
            "artifact_prefix": "stado://weles-captures/test-batch/example/composition/",
            "viewport": {"width": 1280, "height": 720, "device_scale_factor": 1},
            "full_page": true,
            "record_seconds": 0,
            "steps": []
        }]
    });
    std::fs::write(file.path(), serde_json::to_vec_pretty(&plan).unwrap()).unwrap();
    file
}

#[test]
fn declaration_is_read_and_every_workload_is_listed() {
    let storage = storage_with_registry(ALLOWED_REGISTRY);
    let output = stado(storage.path(), &["workload", "list", "--json"]);
    assert!(output.status.success(), "list failed: {}", stderr(&output));
    let document: serde_json::Value = serde_json::from_str(&stdout(&output)).unwrap();
    assert_eq!(document["schema_version"], 1);
    let kinds = document["workloads"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["kind"].as_str().unwrap())
        .collect::<std::collections::BTreeSet<_>>();
    let expected = [
        "gui-automation",
        "jeden-session",
        "mobile-runtime",
        "weles-activity",
        "weles-api-runtime",
        "weles-browser-runtime",
        "weles-browser-task",
        "weles-capture",
        "weles-diagnostics",
        "weles-image-inspect",
        "weles-recordings",
    ]
    .into_iter()
    .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(kinds, expected);
    assert_eq!(
        std::fs::read_to_string(storage.path().join("registry.json")).unwrap(),
        ALLOWED_REGISTRY,
        "listing the declaration does not rewrite fleet state"
    );
}

#[test]
fn unknown_kind_is_refused_by_the_declaration_reader() {
    let storage = storage_with_registry(ALLOWED_REGISTRY);
    let output = stado(storage.path(), &["workload", "run", "not-a-workload"]);
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains(
            "workload kind 'not-a-workload' is not declared; add it to stado-rs/data/workloads.json"
        ),
        "got: {}",
        stderr(&output)
    );
}

#[test]
fn target_without_registry_allowance_is_refused_before_contact() {
    let storage = storage_with_registry(REFUSING_REGISTRY);
    let plan = valid_capture_plan("plain");
    let path = plan.path().to_str().unwrap();
    let output = stado(
        storage.path(),
        &[
            "workload",
            "run",
            "weles-capture",
            "--target",
            "plain",
            "--plan",
            path,
        ],
    );
    assert!(!output.status.success());
    assert!(
        stderr(&output)
            .contains("plain declares no weles-capture; add it to stado-rs/data/workloads.json"),
        "got: {}",
        stderr(&output)
    );
    assert_eq!(
        std::fs::read_to_string(storage.path().join("registry.json")).unwrap(),
        REFUSING_REGISTRY
    );
}

#[test]
fn wrong_plan_schema_refuses_the_whole_plan_without_enqueueing() {
    let storage = storage_with_registry(ALLOWED_REGISTRY);
    let plan = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(
        plan.path(),
        br#"{"schema":"wisent.not-capture.v1","captures":[{"site_slug":"first"}]}"#,
    )
    .unwrap();
    let before = std::fs::read_to_string(storage.path().join("registry.json")).unwrap();
    let output = stado(
        storage.path(),
        &[
            "workload",
            "run",
            "weles-capture",
            "--target",
            "worker",
            "--plan",
            plan.path().to_str().unwrap(),
        ],
    );
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains(
            "weles-capture plan declares schema wisent.not-capture.v1, not wisent.weles-capture-plan.v1; fix the whole plan before any work is enqueued"
        ),
        "got: {}",
        stderr(&output)
    );
    assert_eq!(
        std::fs::read_to_string(storage.path().join("registry.json")).unwrap(),
        before,
        "a refused plan did not mutate the persisted declaration"
    );
    let names = std::fs::read_dir(storage.path())
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    assert_eq!(names, vec!["registry.json"], "nothing was enqueued");
}

#[test]
fn interactive_workload_refuses_json_and_names_attach() {
    let storage = storage_with_registry(ALLOWED_REGISTRY);
    let output = stado(
        storage.path(),
        &["workload", "run", "jeden-session", "--json"],
    );
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains(
            "jeden-session is interactive and cannot produce JSON; use `stado workload attach jeden-session`"
        ),
        "got: {}",
        stderr(&output)
    );
}

#[test]
fn retired_host_verbs_are_absent_from_help() {
    let storage = storage_with_registry(ALLOWED_REGISTRY);
    let output = stado(storage.path(), &["host", "--help"]);
    assert!(
        output.status.success(),
        "host help failed: {}",
        stderr(&output)
    );
    let help = stdout(&output);
    for retired in [
        "jeden-connect",
        "weles-capture",
        "weles-capture-status",
        "weles-browser-task",
        "weles-browser-runtime",
        "weles-run-diagnostics",
        "weles-image-inspect",
        "weles-activity",
        "weles-api-runtime",
        "weles-recordings-dir",
        "mobile-runtime",
        "gui-automation",
    ] {
        assert!(
            !help.contains(retired),
            "retired host verb {retired} remains in help:\n{help}"
        );
    }
}
