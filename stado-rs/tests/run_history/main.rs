//! Real coordinator journey for retaining a terminal job from a migrated run.
//!
//! The fixture recreates the historical boundary: a durable v3 run names a job
//! that reached a terminal prefix before terminal jobs carried submission-linkage
//! fields. The built Stado binary must retain that exact outcome and reap its
//! lifecycle blob instead of terminating the coordinator.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::{json, Value};

const RUN_ID: &str = "migrated-terminal-retention";

struct Journey {
    home: tempfile::TempDir,
    storage: PathBuf,
}

impl Journey {
    fn new() -> Self {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/run-history-runs");
        fs::create_dir_all(&root).unwrap();
        let home = tempfile::Builder::new()
            .prefix("run-history-")
            .tempdir_in(root)
            .unwrap();
        let storage = home.path().join("store");
        fs::create_dir_all(&storage).unwrap();
        let hostname =
            String::from_utf8(Command::new("hostname").arg("-f").output().unwrap().stdout)
                .unwrap()
                .trim()
                .to_ascii_lowercase();
        let platform = if cfg!(target_os = "macos") {
            "darwin-arm64"
        } else {
            "linux-amd64"
        };
        let registry = json!({
            "schema_version": 2,
            "targets": [{
                "name": "run-history-runner",
                "kind": "local",
                "ssh": "nobody@127.0.0.1",
                "release_platform": platform,
                "hostnames": [hostname],
                "disk_cleanup": {
                    "mode": "off",
                    "check_interval_seconds": 300,
                    "low_free_gb": 8,
                    "target_free_gb": 12,
                    "max_bytes_per_pass": 1073741824_u64,
                    "max_items_per_pass": 10,
                    "max_scan_items": 1000,
                    "cleaners": {}
                }
            }],
            "coordinators": [{
                "name": "run-history-coordinator",
                "runtime": "cron",
                "interval_seconds": 60,
                "active": true
            }]
        });
        fs::write(
            storage.join("registry.json"),
            serde_json::to_string_pretty(&registry).unwrap(),
        )
        .unwrap();
        Self { home, storage }
    }

    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_stado"));
        command
            .env_clear()
            .env("HOME", self.home.path())
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("STADO_CONFIG", self.home.path().join("no-config.json"))
            .env("WC_STORAGE_BACKEND", "local")
            .env("WC_LOCAL_STORAGE_PATH", &self.storage)
            .env("WC_PROVIDERS", "local")
            .env("WC_VAST_AUTO_LIST", "false");
        command
    }

    fn invoke(&self, args: &[&str]) -> Output {
        self.command().args(args).output().unwrap()
    }

    fn invoke_ok(&self, args: &[&str]) -> Output {
        let output = self.invoke(args);
        assert!(
            output.status.success(),
            "stado {} failed\nstdout:\n{}\nstderr:\n{}",
            args.join(" "),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
        output
    }

    fn run_path(&self) -> PathBuf {
        self.storage.join("runs").join(format!("{RUN_ID}.json"))
    }
}

#[test]
#[ignore = "Probierz records the real coordinator retention journey"]
fn coordinator_retains_an_unlinked_legacy_terminal_job_from_its_manifest_entry() {
    let journey = Journey::new();
    journey.invoke_ok(&[
        "submit",
        "--run-id",
        RUN_ID,
        "--provider",
        "local",
        "printf migrated-terminal",
    ]);

    let manifest: Value = serde_json::from_slice(&fs::read(journey.run_path()).unwrap()).unwrap();
    let entry = &manifest["entries"][0];
    let job_id = entry["job_id"].as_str().unwrap();
    let mut terminal = entry["planned_job"].clone();
    let terminal_object = terminal.as_object_mut().unwrap();
    terminal_object.insert("state".into(), Value::from("completed"));
    terminal_object.insert(
        "completed_at".into(),
        Value::from("2026-09-04T17:00:00+00:00"),
    );
    terminal_object.insert("run_id".into(), Value::from(""));
    terminal_object.insert("submission_request_digest".into(), Value::from(""));
    terminal_object.insert("submission_command_index".into(), Value::Null);

    let completed_dir = journey.storage.join("completed");
    fs::create_dir_all(&completed_dir).unwrap();
    fs::write(
        completed_dir.join(format!("{job_id}.json")),
        serde_json::to_string_pretty(&terminal).unwrap(),
    )
    .unwrap();
    fs::remove_file(journey.storage.join("queue").join(format!("{job_id}.json"))).unwrap();

    journey.invoke_ok(&["coordinator", "--once"]);

    let retained: Value = serde_json::from_slice(&fs::read(journey.run_path()).unwrap()).unwrap();
    assert_eq!(retained["entries"][0]["state"], "reaped");
    assert_eq!(retained["entries"][0]["outcome"]["prefix"], "completed");
    assert_eq!(retained["entries"][0]["outcome"]["job"]["job_id"], job_id);
    assert!(retained["reaped_at"].is_string());
    assert!(retained["cleanup_completed_at"].is_string());
    assert!(
        !completed_dir.join(format!("{job_id}.json")).exists(),
        "a retained terminal lifecycle blob must be reaped"
    );
}
