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
    "public_origins": [],
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

    /// One finished build job where the queue keeps them. The run object
    /// records no duration of its own, so this is the only clock a release
    /// has, and `release status` joins the two.
    fn seed_completed_job(&self, job_id: &str, started_at: &str, completed_at: &str) {
        let dir = self.storage.path().join("completed");
        std::fs::create_dir_all(&dir).unwrap();
        let job = json!({
            "job_id": job_id,
            "state": "completed",
            "command": "bash deploy/release/build_stado.sh",
            "created_at": started_at,
            "started_at": started_at,
            "completed_at": completed_at,
        });
        std::fs::write(
            dir.join(format!("{job_id}.json")),
            serde_json::to_vec(&job).unwrap(),
        )
        .unwrap();
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

    let out = store.stado(&["registry", "pull", "--path", "targets.w1"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let target: Value = serde_json::from_str(&stdout(&out)).expect("a subtree prints as JSON");
    assert_eq!(target["hostnames"][0], "w1.local");

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

    let out = store.stado(&["registry", "pull", "--path", "schema_version.more"]);
    assert!(!out.status.success());
    assert!(
        stderr(&out)
            .contains("registry value at `schema_version` is a number, which has no `more` inside"),
        "{}",
        stderr(&out)
    );

    let out = store.stado(&["registry", "pull", "--path", "nope"]);
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("registry has no `nope` under `<root>`; keys there: coordinators, public_origins, schema_version, targets"),
        "{}",
        stderr(&out)
    );
}

/// One host's declaration, which is the question an operator actually has.
/// Until 2026-09-20 it needed either the whole document in a file or a dotted
/// path only a reader of the schema could write.
#[test]
fn one_host_is_read_by_name_and_an_unknown_one_is_refused_with_the_names() {
    let store = Store::new();
    let out = store.stado(&["registry", "host", "show", "w2"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let host: Value = serde_json::from_str(&stdout(&out)).expect("the host prints as JSON");
    assert_eq!(host["ssh"], "u@10.0.0.2");
    assert_eq!(host["release_platform"], "darwin-arm64");

    let out = store.stado(&["registry", "host", "show", "w1", "--path", "hostnames.0"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(stdout(&out).trim(), "w1.local", "a string prints bare");

    let out = store.stado(&["registry", "host", "show", "w9"]);
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("registry has no host `w9`; hosts there: w1, w2"),
        "{}",
        stderr(&out)
    );

    let out = store.stado(&["registry", "host", "show", "w1", "--path", "space"]);
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("registry has no `space` under `<root>`; keys there:"),
        "{}",
        stderr(&out)
    );
    untouched(store.home.path());
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

#[test]
fn a_finished_build_reports_what_it_cost() {
    let store = Store::new();
    let run_id = "dddd4444dddd4444dddd4444dddd4444";
    store.seed_run(run_id, "lake", "0.2.4", "completed");
    store.seed_completed_job(
        &format!("job-{run_id}"),
        "2026-09-19T21:04:30+00:00",
        "2026-09-19T21:23:04+00:00",
    );

    let out = store.stado(&["release", "status", "--run", "dddd4444"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let text = stdout(&out);
    assert!(
        text.contains("linux-amd64 published job=job-dddd [completed] took 18m34s"),
        "the platform line carries the job's own clock: {text}"
    );

    let out = store.stado(&["release", "status", "--run", "dddd4444", "--json"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let runs: Value = serde_json::from_str(&stdout(&out)).unwrap();
    assert_eq!(
        runs["runs"][0]["platforms"]["linux-amd64"]["build_seconds"],
        json!(1114),
        "the machine answer carries the seconds, not a formatted string"
    );
    untouched(store.home.path());
}

#[test]
fn a_build_whose_job_the_queue_no_longer_holds_reports_no_cost() {
    let store = Store::new();
    store.seed_run(
        "eeee5555eeee5555eeee5555eeee5555",
        "lake",
        "0.2.5",
        "completed",
    );

    let out = store.stado(&["release", "status", "--run", "eeee5555"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let text = stdout(&out);
    assert!(text.contains("eeee5555 lake 0.2.5"), "{text}");
    assert!(
        !text.contains("took"),
        "a run whose job is gone invents no duration: {text}"
    );
    untouched(store.home.path());
}

#[test]
fn one_field_is_written_by_the_path_that_reads_it() {
    let store = Store::new();
    let written = store.stado(&[
        "registry",
        "set",
        "--path",
        "targets.w2.ssh",
        "--value",
        "u@10.0.0.9",
    ]);
    assert!(written.status.success(), "{}", stderr(&written));
    assert!(
        stdout(&written).contains("was u@10.0.0.2"),
        "the sentence names what it replaced: {}",
        stdout(&written)
    );

    let read = store.stado(&["registry", "pull", "--path", "targets.w2.ssh"]);
    assert_eq!(stdout(&read).trim(), "u@10.0.0.9", "{}", stderr(&read));

    // A second identical write changes nothing and says so.
    let again = store.stado(&[
        "registry",
        "set",
        "--path",
        "targets.w2.ssh",
        "--value",
        "u@10.0.0.9",
        "--json",
    ]);
    assert!(again.status.success(), "{}", stderr(&again));
    let receipt: Value = serde_json::from_str(&stdout(&again)).expect("a receipt");
    assert_eq!(receipt["state"], "unchanged");
    assert_eq!(receipt["schema"], "stado.registry-set-receipt.v1");
    untouched(store.home.path());
}

#[test]
fn a_write_to_a_path_that_does_not_exist_changes_nothing() {
    let store = Store::new();
    let before = std::fs::read_to_string(store.storage.path().join("registry.json"))
        .expect("read the canonical document");
    let refused = store.stado(&[
        "registry",
        "set",
        "--path",
        "targets.w9.release_platform",
        "--value",
        "linux-amd64",
    ]);
    assert!(!refused.status.success());
    assert!(
        stderr(&refused)
            .contains("registry array `targets` has no element named `w9`; names there: w1, w2"),
        "{}",
        stderr(&refused)
    );
    let after = std::fs::read_to_string(store.storage.path().join("registry.json"))
        .expect("read the canonical document");
    assert_eq!(before, after, "a refused write left the registry alone");
}
