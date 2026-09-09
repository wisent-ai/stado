//! The isolated journey: a canonical registry naming this machine, the
//! product invocation against its own store, the real agent this case
//! starts, and the reads that say what the store holds.

use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::{build_platform, RECIPE};

pub(crate) struct Journey {
    pub(crate) home: tempfile::TempDir,
    pub(crate) storage: PathBuf,
    agent: Option<Child>,
}

impl Journey {
    pub(crate) fn new() -> Self {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/build-runs");
        fs::create_dir_all(&root).unwrap();
        let home = tempfile::Builder::new()
            .prefix("build-")
            .tempdir_in(root)
            .unwrap();
        let storage = home.path().join("store");
        fs::create_dir_all(&storage).unwrap();
        let hostname =
            String::from_utf8(Command::new("hostname").arg("-f").output().unwrap().stdout)
                .unwrap()
                .trim()
                .to_ascii_lowercase();
        let registry = json!({
            "schema_version": 2,
            "targets": [{
                "name": "build-runner",
                "kind": "local",
                "ssh": "nobody@127.0.0.1",
                "release_platform": build_platform(),
                "hostnames": [hostname],
                "disk_cleanup": {
                    "mode": "off",
                    "check_interval_seconds": 300,
                    "low_free_gb": 10,
                    "target_free_gb": 12,
                    "max_bytes_per_pass": 53687091200_u64,
                    "max_items_per_pass": 50,
                    "max_scan_items": 10000,
                    "cleaners": {}
                }
            }],
            "coordinators": [{
                "name": "build-coordinator",
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
        Self {
            home,
            storage,
            agent: None,
        }
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

    pub(crate) fn invoke(&self, args: &[&str]) -> Output {
        self.command().args(args).output().unwrap()
    }

    pub(crate) fn invoke_ok(&self, args: &[&str]) -> Output {
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

    pub(crate) fn start_agent(&mut self) {
        let stdout = File::create(self.home.path().join("agent.out")).unwrap();
        let stderr = File::create(self.home.path().join("agent.err")).unwrap();
        self.agent = Some(
            self.command()
                .args(["agent", "--target", "build-runner"])
                .stdout(Stdio::from(stdout))
                .stderr(Stdio::from(stderr))
                .spawn()
                .unwrap(),
        );
        let deadline = Instant::now() + Duration::from_secs(30);
        while Instant::now() < deadline {
            if fs::read_dir(self.storage.join("capacity"))
                .ok()
                .and_then(|mut entries| entries.next())
                .is_some()
            {
                return;
            }
            if self.agent.as_mut().unwrap().try_wait().unwrap().is_some() {
                break;
            }
            thread::sleep(Duration::from_millis(100));
        }
        panic!(
            "agent published no capacity: {}",
            fs::read_to_string(self.home.path().join("agent.err")).unwrap_or_default()
        );
    }

    pub(crate) fn status(&self) -> Value {
        let output = self.invoke_ok(&["builds", "status", RECIPE, "--json"]);
        serde_json::from_slice(&output.stdout).unwrap()
    }

    pub(crate) fn wait_for_terminal_job(&mut self, job_id: &str) {
        let deadline = Instant::now() + Duration::from_secs(180);
        while Instant::now() < deadline {
            if ["completed", "uploaded", "failed"].iter().any(|prefix| {
                self.storage
                    .join(prefix)
                    .join(format!("{job_id}.json"))
                    .exists()
            }) {
                return;
            }
            if self.agent.as_mut().unwrap().try_wait().unwrap().is_some() {
                break;
            }
            thread::sleep(Duration::from_millis(100));
        }
        panic!(
            "build job {job_id} did not finish: {}",
            fs::read_to_string(self.home.path().join("agent.err")).unwrap_or_default()
        );
    }
}

impl Drop for Journey {
    fn drop(&mut self) {
        if let Some(agent) = self.agent.as_mut() {
            let _ = agent.kill();
            let _ = agent.wait();
        }
    }
}
