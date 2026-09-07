//! Declared repair capability tests against an isolated local registry.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const EMPTY_REGISTRY: &str = r#"{"schema_version": 2, "targets": [], "coordinators": []}"#;
const CATALOG: &str = include_str!("../../data/service-catalog.json");

fn stado(storage: &Path, args: &[&str]) -> Output {
    stado_with_env(storage, args, None)
}

fn stado_with_env(storage: &Path, args: &[&str], extra: Option<(&str, &str)>) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_stado"));
    command
        .args(args)
        .env("WC_STORAGE_BACKEND", "local")
        .env("WC_LOCAL_STORAGE_PATH", storage)
        .env("STADO_CONFIG", storage.join("no-such-config.json"))
        .env_remove("COMPUTE_API_KEY")
        .env_remove("COMPUTE_API_URL")
        .env_remove("WC_PROFILES_DIR");
    if let Some((name, value)) = extra {
        command.env(name, value);
    }
    command.output().expect("stado binary runs")
}

fn storage() -> tempfile::TempDir {
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(directory.path().join("registry.json"), EMPTY_REGISTRY).unwrap();
    directory
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn json(output: &Output) -> serde_json::Value {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "stdout was not JSON: {error}\nstdout={}\nstderr={}",
            stdout(output),
            stderr(output)
        )
    })
}

fn snapshot(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn visit(root: &Path, directory: &Path, files: &mut BTreeMap<PathBuf, Vec<u8>>) {
        let mut entries = std::fs::read_dir(directory)
            .unwrap()
            .map(Result::unwrap)
            .collect::<Vec<_>>();
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let path = entry.path();
            if entry.file_type().unwrap().is_dir() {
                visit(root, &path, files);
            } else {
                files.insert(
                    path.strip_prefix(root).unwrap().to_path_buf(),
                    std::fs::read(path).unwrap(),
                );
            }
        }
    }

    let mut files = BTreeMap::new();
    visit(root, root, &mut files);
    files
}

#[test]
fn list_reads_every_step_from_the_compiled_service_catalog() {
    let storage = storage();
    let before = snapshot(storage.path());
    let output = stado(storage.path(), &["repair", "list", "--json"]);
    assert!(output.status.success(), "{}", stderr(&output));

    let report = json(&output);
    assert_eq!(report["declaration"], "stado-rs/data/service-catalog.json");
    let declaration: serde_json::Value = serde_json::from_str(CATALOG).unwrap();
    let expected = declaration["services"].as_array().unwrap();
    let actual = report["services"].as_array().unwrap();
    assert_eq!(actual.len(), expected.len());

    let mut declared_steps = 0;
    for expected_service in expected {
        let name = expected_service["name"].as_str().unwrap();
        let actual_service = actual
            .iter()
            .find(|service| service["name"] == name)
            .unwrap_or_else(|| panic!("list omitted declared service {name}"));
        assert_eq!(actual_service["repair"], expected_service["repair"]);
        declared_steps += expected_service["repair"].as_array().unwrap().len();
    }
    assert_eq!(declared_steps, 13);
    assert_eq!(snapshot(storage.path()), before);
}

#[test]
fn show_reads_the_selected_step_from_the_compiled_catalog() {
    let storage = storage();
    let before = snapshot(storage.path());
    let output = stado(
        storage.path(),
        &["repair", "show", "stado", "link", "--json"],
    );
    assert!(output.status.success(), "{}", stderr(&output));
    let report = json(&output);
    assert_eq!(report["declaration"], "stado-rs/data/service-catalog.json");
    assert_eq!(report["service"], "stado");
    let declaration: serde_json::Value = serde_json::from_str(CATALOG).unwrap();
    let expected = declaration["services"]
        .as_array()
        .unwrap()
        .iter()
        .find(|service| service["name"] == "stado")
        .unwrap()["repair"]
        .as_array()
        .unwrap()
        .iter()
        .find(|step| step["name"] == "link")
        .unwrap();
    assert_eq!(&report["step"], expected);
    assert_eq!(snapshot(storage.path()), before);
}

#[test]
fn undeclared_service_is_refused_by_name_and_declaration() {
    let storage = storage();
    let before = snapshot(storage.path());
    let output = stado(storage.path(), &["repair", "unknown-service", "--json"]);
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains(
            "unknown-service declares no repair; add it to stado-rs/data/service-catalog.json."
        ),
        "{}",
        stderr(&output)
    );
    assert_eq!(snapshot(storage.path()), before);
}

#[test]
fn undeclared_step_is_refused_by_name_and_declaration() {
    let storage = storage();
    let before = snapshot(storage.path());
    let output = stado(
        storage.path(),
        &["repair", "stado", "--step", "unknown-step", "--json"],
    );
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains(
            "stado declares no repair step unknown-step; add it to stado-rs/data/service-catalog.json."
        ),
        "{}",
        stderr(&output)
    );
    assert_eq!(snapshot(storage.path()), before);
}

#[test]
fn dry_run_reports_every_declared_step_without_mutating_storage() {
    let storage = storage();
    let before = snapshot(storage.path());
    let output = stado(storage.path(), &["repair", "stado", "--json"]);
    assert!(output.status.success(), "{}", stderr(&output));

    let report = json(&output);
    assert_eq!(report["service"], "stado");
    assert_eq!(report["applied"], false);
    assert!(report["target"].is_null());
    let steps = report["steps"].as_array().unwrap();
    let declaration: serde_json::Value = serde_json::from_str(CATALOG).unwrap();
    let expected = declaration["services"]
        .as_array()
        .unwrap()
        .iter()
        .find(|service| service["name"] == "stado")
        .unwrap()["repair"]
        .as_array()
        .unwrap();
    assert_eq!(steps.len(), expected.len());
    for declared in expected {
        let name = declared["name"].as_str().unwrap();
        let step = steps
            .iter()
            .find(|step| step["name"] == name)
            .unwrap_or_else(|| panic!("dry run omitted declared step {name}"));
        assert_eq!(step["status"], "planned");
        assert_eq!(step["summary"], declared["summary"]);
        assert_eq!(step["proof"], declared["proof"]);
        assert_eq!(step["mutating"], declared["mutating"]);
        assert_eq!(step["observation"]["status"], "target_not_selected");
    }
    assert_eq!(snapshot(storage.path()), before);
}

#[test]
fn declared_step_with_missing_implementation_is_refused_not_skipped() {
    let storage = storage();
    let before = snapshot(storage.path());
    let output = stado_with_env(
        storage.path(),
        &["repair", "list", "--json"],
        Some((
            "STADO_REPAIR_TEST_MISSING_IMPLEMENTATION",
            "stado:object-api",
        )),
    );
    assert!(!output.status.success());
    assert!(
        stdout(&output).is_empty(),
        "a mismatch must not return a partial list"
    );
    assert!(
        stderr(&output).contains(
            "stado repair step object-api declares no implementation; add it to stado-rs/src/cli/repair.rs."
        ),
        "{}",
        stderr(&output)
    );
    assert_eq!(snapshot(storage.path()), before);
}

#[test]
fn service_with_no_declared_steps_is_refused_with_the_catalog_path() {
    let storage = storage();
    let before = snapshot(storage.path());
    let output = stado(storage.path(), &["repair", "brama", "--json"]);
    assert!(!output.status.success());
    assert!(stdout(&output).is_empty());
    assert!(
        stderr(&output).contains(
            "brama declares no repair steps; add them to stado-rs/data/service-catalog.json."
        ),
        "{}",
        stderr(&output)
    );
    assert_eq!(snapshot(storage.path()), before);
}

#[test]
fn apply_without_a_target_is_refused_with_the_required_flag() {
    let storage = storage();
    let before = snapshot(storage.path());
    let output = stado(storage.path(), &["repair", "stado", "--apply", "--json"]);
    assert!(!output.status.success());
    assert!(stdout(&output).is_empty());
    assert!(
        stderr(&output).contains(
            "stado declares mutating repair steps but no target was selected; pass --target <TARGET>."
        ),
        "{}",
        stderr(&output)
    );
    assert_eq!(snapshot(storage.path()), before);
}

#[test]
fn host_help_contains_none_of_the_replaced_repair_verbs() {
    let storage = storage();
    let output = stado(storage.path(), &["host", "--help"]);
    assert!(output.status.success(), "{}", stderr(&output));
    let help = stdout(&output);
    for verb in [
        "recover",
        "recover-object-api",
        "recover-skarbiec-audit",
        "recover-skarbiec-crypto",
        "recover-skarbiec-acquisition-state",
        "repair-link",
        "repair-release-store",
        "reconcile",
        "reconcile-agent-skarbiec",
        "reconcile-object-verifier",
        "reconcile-release-verifier",
        "reconcile-service-verifier",
        "storage-root-reconcile",
    ] {
        assert!(
            !help.lines().any(|line| line.trim_start().starts_with(verb)),
            "host help still contains {verb}:\n{help}"
        );
    }
}
