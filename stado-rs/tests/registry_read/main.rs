//! Two questions the CLI answers directly since 2026-09-19, so the answer is
//! not a file in `~/.oko` grepped afterwards: one part of the registry
//! (`stado registry pull --path`) and one release run (`stado release status
//! --run | --version`).
//!
//! Every test drives the built `stado` binary against a local storage
//! backend under a tempdir, with HOME and STADO_CONFIG isolated the way
//! `registry_cache` isolates them, so the operator's registry, cache and
//! credentials are never read or written.

use std::path::Path;
use std::process::{Command, Output};

use serde_json::{json, Value};

const REGISTRY: &str = r#"{
    "schema_version": 2,
    "coordinators": [],
    "release_control": {
        "schema_version": 1,
        "generation": 3,
        "trusted_keys": {},
        "products": {
            "lake": { "desired": { "version": "0.2.3" }, "targets": {} }
        }
    },
    "targets": [
        {
            "name": "w1",
            "kind": "local",
            "ssh": "u@10.0.0.1",
            "release_platform": "linux-amd64",
            "hostnames": ["w1.local"]
        },
        {
            "name": "w2",
            "kind": "local",
            "ssh": "u@10.0.0.2",
            "release_platform": "darwin-arm64",
            "hostnames": ["w2.local"]
        }
    ]
}"#;

struct Store {
    home: tempfile::TempDir,
    storage: tempfile::TempDir,
}

impl Store {
    fn new() -> Self {
        let store = Self {
            home: tempfile::tempdir().unwrap(),
            storage: tempfile::tempdir().unwrap(),
        };
        std::fs::write(store.storage.path().join("registry.json"), REGISTRY).unwrap();
        store
    }

    /// One run object where `stado release submit` keeps them.
    fn seed_run(&self, run_id: &str, product: &str, version: &str, state: &str) {
        let dir = self
            .storage
            .path()
            .join("runs/release-pipeline")
            .join(run_id);
        std::fs::create_dir_all(&dir).unwrap();
        let failure = match state {
            "failed" => json!("required delivery w2 failed: workload exited unsuccessfully"),
            _ => Value::Null,
        };
        let run = json!({
            "schema_version": 1,
            "run_id": run_id,
            "product": product,
            "version": version,
            "channel": "candidate",
            "state": state,
            "platforms": {
                "linux-amd64": {
                    "platform": "linux-amd64",
                    "builder": "w1",
                    "job_id": format!("job-{run_id}"),
                    "output_prefix": format!("status/job-{run_id}/output/"),
                    "state": "published"
                }
            },
            "deliveries": {},
            "failure": failure
        });
        std::fs::write(dir.join("run.json"), serde_json::to_vec(&run).unwrap()).unwrap();
    }

    fn stado(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_stado"))
            .args(args)
            .env("HOME", self.home.path())
            .env("WC_STORAGE_BACKEND", "local")
            .env("WC_LOCAL_STORAGE_PATH", self.storage.path())
            .env(
                "STADO_CONFIG",
                self.storage.path().join("no-such-config.json"),
            )
            .env_remove("COMPUTE_API_KEY")
            .env_remove("COMPUTE_API_URL")
            .env_remove("WC_PROFILES_DIR")
            .output()
            .expect("stado binary runs")
    }
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn untouched(path: &Path) {
    assert!(
        !path.join(".oko").exists() && !path.join(".stado").join("work").exists(),
        "a read wrote under {}",
        path.display()
    );
}

#[test]
fn one_part_of_the_registry_is_printed_by_key_index_or_name() {
    let store = Store::new();
    let out = store.stado(&["registry", "pull", "--path", "targets.w2.release_platform"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(stdout(&out).trim(), "darwin-arm64", "a string prints bare");

    let out = store.stado(&[
        "registry",
        "pull",
        "--path",
        "release_control.products.lake.desired",
    ]);
    assert!(out.status.success(), "{}", stderr(&out));
    let desired: Value = serde_json::from_str(&stdout(&out)).expect("a subtree prints as JSON");
    assert_eq!(desired["version"], "0.2.3");

    let out = store.stado(&["registry", "pull", "--path", "targets.1.name"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(
        stdout(&out).trim(),
        "w2",
        "an index reaches an array element"
    );
    untouched(store.home.path());
}

#[test]
fn a_missing_segment_is_refused_with_what_exists_there() {
    let store = Store::new();
    let out = store.stado(&["registry", "pull", "--path", "targets.w9.ssh"]);
    assert!(!out.status.success());
    assert!(
        stderr(&out)
            .contains("registry array `targets` has no element named `w9`; names there: w1, w2"),
        "{}",
        stderr(&out)
    );

    let out = store.stado(&[
        "registry",
        "pull",
        "--path",
        "release_control.generation.more",
    ]);
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("registry value at `release_control.generation` is a number, which has no `more` inside"),
        "{}",
        stderr(&out)
    );

    let out = store.stado(&["registry", "pull", "--path", "nope"]);
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("registry has no `nope` under `<root>`; keys there: coordinators, release_control, schema_version, targets"),
        "{}",
        stderr(&out)
    );
}

#[test]
fn one_release_run_is_read_by_id_prefix_or_version() {
    let store = Store::new();
    store.seed_run(
        "aaaa1111aaaa1111aaaa1111aaaa1111",
        "lake",
        "0.2.2",
        "failed",
    );
    store.seed_run(
        "bbbb2222bbbb2222bbbb2222bbbb2222",
        "lake",
        "0.2.3",
        "completed",
    );
    store.seed_run(
        "cccc3333cccc3333cccc3333cccc3333",
        "other",
        "1.0.0",
        "completed",
    );

    let out = store.stado(&["release", "status", "--run", "aaaa1111"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let text = stdout(&out);
    assert!(
        text.contains("run aaaa1111 lake 0.2.2 candidate failed"),
        "{text}"
    );
    assert!(
        text.contains("failure: required delivery w2 failed"),
        "{text}"
    );
    assert!(!text.contains("bbbb2222"), "only the named run: {text}");
    assert!(
        !text.contains("target="),
        "no target rows for a run question: {text}"
    );

    let out = store.stado(&["release", "status", "lake", "--version", "0.2.3", "--json"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let runs: Value = serde_json::from_str(&stdout(&out)).unwrap();
    let ids: Vec<&str> = runs["runs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|run| run["run_id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, vec!["bbbb2222bbbb2222bbbb2222bbbb2222"]);
    untouched(store.home.path());
}

#[test]
fn an_unknown_run_is_refused_naming_the_newest_runs() {
    let store = Store::new();
    store.seed_run(
        "bbbb2222bbbb2222bbbb2222bbbb2222",
        "lake",
        "0.2.3",
        "completed",
    );
    let out = store.stado(&["release", "status", "--run", "ffff"]);
    assert!(!out.status.success());
    let text = stderr(&out);
    assert!(
        text.contains("no release run matches run=ffff version=* product=* among the newest 120 runs; the newest are:"),
        "{text}"
    );
    assert!(
        text.contains("bbbb2222bbbb2222bbbb2222bbbb2222 lake 0.2.3"),
        "{text}"
    );
}
