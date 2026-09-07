//! Declared runner capability tests against an isolated local registry.

use std::path::Path;
use std::process::{Command, Output};

fn stado(storage: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_stado"))
        .args(args)
        .env("WC_STORAGE_BACKEND", "local")
        .env("WC_LOCAL_STORAGE_PATH", storage)
        .env("STADO_CONFIG", storage.join("no-such-config.json"))
        .env_remove("COMPUTE_API_KEY")
        .env_remove("COMPUTE_API_URL")
        .env_remove("WC_PROFILES_DIR")
        .output()
        .expect("stado binary runs")
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn storage_with_registry(document: &str) -> tempfile::TempDir {
    let directory = tempfile::tempdir().expect("temp storage");
    std::fs::write(directory.path().join("registry.json"), document).expect("seed registry");
    directory
}

fn persisted_registry(storage: &Path) -> String {
    std::fs::read_to_string(storage.join("registry.json")).expect("registry remains")
}

const EMPTY_REGISTRY: &str = r#"{"schema_version":2,"targets":[],"coordinators":[]}"#;

const REGISTRY_WITH_OFFLINE_HOST: &str = r#"{"schema_version":2,"targets":[{"name":"runner-fixture","kind":"local","release_platform":"darwin-arm64","hostnames":["runner-fixture.invalid"]}],"coordinators":[]}"#;

const REGISTRY_WITH_NONLOCAL_TARGET: &str = r#"{"schema_version":2,"targets":[{"name":"runner-cloud","kind":"gcp","release_platform":"linux-amd64","ssh":"runner-cloud.invalid","hostnames":["runner-cloud.invalid"]}],"coordinators":[]}"#;

#[test]
fn list_reads_every_profile_from_the_compiled_declaration() {
    let storage = storage_with_registry(EMPTY_REGISTRY);
    let output = stado(storage.path(), &["runner", "list", "--json"]);
    assert!(
        output.status.success(),
        "runner list failed: {}",
        stderr(&output)
    );
    let document: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("runner list JSON");
    assert_eq!(document["schema_version"], 1);
    assert_eq!(document["profiles"][0]["name"], "precheck");
    assert_eq!(document["profiles"][0]["slug"], "stado-precheck");
    assert_eq!(
        document["profiles"][0]["installers"]["darwin-arm64"],
        "kronika-launchd"
    );
    assert_eq!(
        document["profiles"][0]["labels"],
        serde_json::json!([
            "stado",
            "stado-precheck",
            "stado-release",
            "stado-publisher",
            "stado-control-plane"
        ])
    );
    assert_eq!(document["profiles"][0]["github_runner_group"], "Default");
    assert_eq!(
        document["profiles"][0]["unit_label"],
        "stado-precheck-runner"
    );
    assert_eq!(document["profiles"][1]["name"], "publisher");
    assert_eq!(
        document["profiles"][1]["secrets"][0], "GITHUB_TOKEN",
        "the declaration carries the secrets instead of a profile-specific command"
    );
    assert_eq!(
        document["profiles"][1]["installers"]["linux-amd64"],
        "publisher-systemd"
    );
    assert_eq!(
        document["profiles"][1]["labels"],
        serde_json::json!(["stado", "stado-publisher"])
    );
    assert_eq!(document["profiles"][1]["github_runner_group"], "Default");
    assert_eq!(
        document["profiles"][1]["unit_label"],
        "stado-publisher-runner"
    );
    assert_eq!(document["profiles"][1]["accepts_repository_scope"], true);
    assert_eq!(
        persisted_registry(storage.path()),
        EMPTY_REGISTRY,
        "a declaration read does not rewrite the fleet"
    );
}

#[test]
fn install_refuses_an_unknown_profile_with_the_declaration_remedy() {
    let storage = storage_with_registry(EMPTY_REGISTRY);
    let output = stado(
        storage.path(),
        &[
            "runner",
            "install",
            "runner-fixture",
            "--profile",
            "missing",
            "--json",
        ],
    );
    assert!(!output.status.success(), "unknown profile was accepted");
    assert!(
        stderr(&output).contains(
            "runner profile 'missing' is not declared; add it to stado-rs/data/runner-profiles.json"
        ),
        "refusal did not name the profile and declaration: {}",
        stderr(&output)
    );
    assert_eq!(persisted_registry(storage.path()), EMPTY_REGISTRY);
}

#[test]
fn install_refuses_an_unknown_target_with_the_registry_sentence() {
    let storage = storage_with_registry(EMPTY_REGISTRY);
    let output = stado(
        storage.path(),
        &[
            "runner",
            "install",
            "ghost",
            "--profile",
            "precheck",
            "--json",
        ],
    );
    assert!(!output.status.success(), "unknown target was accepted");
    assert!(
        stderr(&output)
            .contains("ghost declares no host target; add it to the canonical fleet registry"),
        "refusal did not name the registry target: {}",
        stderr(&output)
    );
    assert_eq!(persisted_registry(storage.path()), EMPTY_REGISTRY);
}

#[test]
fn install_refuses_a_host_without_a_reachable_registry_destination() {
    let storage = storage_with_registry(REGISTRY_WITH_OFFLINE_HOST);
    let output = stado(
        storage.path(),
        &[
            "runner",
            "install",
            "runner-fixture",
            "--profile",
            "precheck",
            "--json",
        ],
    );
    assert!(!output.status.success(), "unreachable host was accepted");
    assert!(
        stderr(&output).contains(
            "runner-fixture declares no reachable host destination; add a registry-managed ssh destination to the canonical fleet registry or run the command on that host"
        ),
        "refusal did not name the missing destination and remedy: {}",
        stderr(&output)
    );
    assert_eq!(
        persisted_registry(storage.path()),
        REGISTRY_WITH_OFFLINE_HOST
    );
}

#[test]
fn install_refuses_a_nonlocal_registry_target() {
    let storage = storage_with_registry(REGISTRY_WITH_NONLOCAL_TARGET);
    let output = stado(
        storage.path(),
        &[
            "runner",
            "install",
            "runner-cloud",
            "--profile",
            "precheck",
            "--json",
        ],
    );
    assert!(!output.status.success(), "nonlocal target was accepted");
    assert!(
        stderr(&output).contains(
            "runner-cloud declares no local host provider; set its kind to a local host capability in the canonical fleet registry or select a local host target"
        ),
        "refusal did not name the missing local provider and remedy: {}",
        stderr(&output)
    );
    assert_eq!(
        persisted_registry(storage.path()),
        REGISTRY_WITH_NONLOCAL_TARGET
    );
}

#[test]
fn fleet_report_keeps_listener_and_job_slot_typed_for_each_profile() {
    let storage = storage_with_registry(REGISTRY_WITH_OFFLINE_HOST);
    let output = stado(storage.path(), &["runner", "report", "--json"]);
    assert!(
        output.status.success(),
        "offline fleet report failed instead of carrying unavailable state: {}",
        stderr(&output)
    );
    let document: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("runner report JSON");
    let profiles = document["hosts"][0]["profiles"]
        .as_array()
        .expect("host profile reports");
    assert_eq!(profiles.len(), 2);
    for profile in profiles {
        assert!(
            profile["listener"].get("connected").is_some(),
            "listener.connected is absent: {profile}"
        );
        assert!(
            profile["listener"]["connected"].is_null()
                || profile["listener"]["connected"].is_boolean(),
            "listener.connected is not typed: {profile}"
        );
        assert!(
            profile["host_job_slot"].is_string(),
            "host_job_slot is absent: {profile}"
        );
    }
    assert_eq!(
        persisted_registry(storage.path()),
        REGISTRY_WITH_OFFLINE_HOST,
        "the fleet-wide read does not mutate its declaration"
    );
}

#[test]
fn repository_scope_is_carried_into_the_exact_installer_program() {
    let program = stado::deploy::host_precheck_runner::installer_program(
        &stado::deploy::host_precheck_runner::InstallerRequest {
            profile_name: "precheck",
            target_name: "runner-fixture",
            platform_name: "darwin-arm64",
            repository: Some("example"),
        },
        "registration-token",
        "http://127.0.0.1:18080",
        18_080,
        false,
    )
    .expect("declared installer renders");
    assert!(
        program.contains("https://github.com/wisent-ai/example"),
        "repository registration URL is absent"
    );
    assert!(
        program.contains("repository:wisent-ai/example"),
        "host registration record does not carry the actual repository scope"
    );
}

#[test]
fn the_model_review_route_request_carries_the_alias_and_its_one_route() {
    let request = stado::deploy::host_precheck_runner::model_review_route_request();
    let body = request.as_object().expect("admin-route body object");
    assert_eq!(
        body.keys().map(String::as_str).collect::<Vec<_>>(),
        ["alias", "primary"],
        "the body carries the alias and its primary route and nothing else. The key \
         for the ordered alternates is deliberately absent: Brama's AdminRouteUpdate \
         declares that field with serde(default) over Vec<String>, so an absent key \
         and an explicit empty list reach update_admin_route as the same empty \
         vector. A third key here is Stado sending what that omission no longer \
         covers: {request}"
    );
    assert_eq!(body["alias"], "wisent-backend/evaluation");
    assert_eq!(body["primary"], "best");
}

#[test]
fn host_help_contains_neither_replaced_runner_group() {
    let storage = storage_with_registry(EMPTY_REGISTRY);
    let output = stado(storage.path(), &["host", "--help"]);
    assert!(
        output.status.success(),
        "host help failed: {}",
        stderr(&output)
    );
    let help = stdout(&output);
    assert!(!help.contains("precheck-runner"), "{help}");
    assert!(!help.contains("publisher-runner"), "{help}");
}
