//! The isolated store every case drives the binary against: its own home, its
//! own storage backend and its own configuration, so the operator registry,
//! cache and credentials are never read or written.

use std::path::Path;
use std::process::{Command, Output};

use serde_json::{json, Value};

pub(crate) const REGISTRY: &str = r#"{
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

pub(crate) struct Store {
    pub(crate) home: tempfile::TempDir,
    pub(crate) storage: tempfile::TempDir,
}

impl Store {
    pub(crate) fn new() -> Self {
        let store = Self {
            home: tempfile::tempdir().unwrap(),
            storage: tempfile::tempdir().unwrap(),
        };
        std::fs::write(store.storage.path().join("registry.json"), REGISTRY).unwrap();
        store
    }

    /// One run object where `stado release submit` keeps them.
    pub(crate) fn seed_run(&self, run_id: &str, product: &str, version: &str, state: &str) {
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
    pub(crate) fn seed_completed_job(&self, job_id: &str, started_at: &str, completed_at: &str) {
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

    pub(crate) fn stado(&self, args: &[&str]) -> Output {
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

pub(crate) fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

pub(crate) fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

pub(crate) fn untouched(path: &Path) {
    assert!(
        !path.join(".oko").exists() && !path.join(".stado").join("work").exists(),
        "a read wrote under {}",
        path.display()
    );
}
