//! The real-run harness for the workload area.
//!
//! It gives each case a fresh tempdir holding the local store, the job working
//! directories (`HOME`) and a `STADO_CONFIG` path that does not exist, and a
//! registry naming *this* machine as a `local` target. The production placement
//! path therefore claims a routed job for the current host and the operating
//! system really executes it: no provider is contacted, no executor is
//! simulated, and no host but this one is ever addressed.

use std::fs;
use std::path::PathBuf;
use std::process::{Command, ExitStatus, Output, Stdio};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

pub const TARGET: &str = "workload-current-host";
const SYSTEM_PATH: &str = "/usr/bin:/bin:/usr/sbin:/sbin";
/// Bound on one `stado agent --auto --idle-shutdown` drain. The agent polls
/// every 10 s and needs its second tick to publish measured CPU capacity, so a
/// healthy drain finishes in ~11-21 s; this deadline only catches a hang.
const AGENT_DEADLINE: Duration = Duration::from_secs(180);

fn platform() -> &'static str {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => "darwin-arm64",
        ("linux", "x86_64") => "linux-amd64",
        (os, arch) => panic!(
            "blocked: the workload journey runs the current host and requires \
             macOS arm64 or Linux amd64, got {os}-{arch}"
        ),
    }
}

fn hostname() -> String {
    let output = Command::new("hostname")
        .env_clear()
        .env("PATH", SYSTEM_PATH)
        .output()
        .expect("blocked: the real hostname executable could not start");
    assert!(
        output.status.success(),
        "blocked: the real hostname executable failed"
    );
    let name = String::from_utf8(output.stdout)
        .expect("the kernel hostname is UTF-8")
        .trim()
        .to_string();
    assert!(!name.is_empty(), "the current host has no hostname");
    name
}

pub fn said(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

pub struct Area {
    pub root: tempfile::TempDir,
    pub storage: PathBuf,
    pub home: PathBuf,
    pub hostname: String,
    config: PathBuf,
}

impl Area {
    pub fn new() -> Self {
        let root = tempfile::Builder::new()
            .prefix("stado-workload-")
            .tempdir()
            .expect("create the isolated workload journey");
        let storage = root.path().join("storage");
        let home = root.path().join("home");
        for directory in [&storage, &home] {
            fs::create_dir_all(directory).expect("create isolated journey directory");
        }
        let hostname = hostname();
        fs::write(
            storage.join("registry.json"),
            serde_json::to_vec_pretty(&registry(&hostname)).unwrap(),
        )
        .expect("write the isolated registry");
        let config = root.path().join("config-that-does-not-exist.json");
        Self {
            root,
            storage,
            home,
            hostname,
            config,
        }
    }

    pub fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_stado"));
        command
            .args(args)
            .env("WC_STORAGE_BACKEND", "local")
            .env("WC_LOCAL_STORAGE_PATH", &self.storage)
            .env("STADO_CONFIG", &self.config)
            .env("HOME", &self.home)
            .env_remove("COMPUTE_API_KEY")
            .env_remove("COMPUTE_API_URL")
            .env_remove("WC_PROFILES_DIR")
            .env_remove("WC_VAST_AUTO_LIST");
        command
    }

    pub fn stado(&self, args: &[&str]) -> Output {
        self.command(args).output().expect("the stado binary runs")
    }

    /// Submit one command routed to this host and return its job id.
    pub fn submit(&self, run_id: &str, command: &str) -> String {
        let output = self.stado(&[
            "submit",
            "--run-id",
            run_id,
            "--pinned-host",
            TARGET,
            command,
        ]);
        assert!(
            output.status.success(),
            "submit failed: {}",
            said(&output.stderr)
        );
        let stdout = said(&output.stdout);
        let receipt: Value = serde_json::from_str(
            stdout
                .lines()
                .find(|line| line.starts_with('{'))
                .unwrap_or_else(|| panic!("submit printed no receipt:\n{stdout}")),
        )
        .expect("the submission receipt is JSON");
        receipt["jobs"][0]["job_id"]
            .as_str()
            .expect("the receipt names the job")
            .to_string()
    }

    /// Run the product's own local agent until it drains the queue and shuts
    /// itself down, and return everything it logged.
    pub fn drain(&self) -> String {
        let log_path = self.root.path().join("agent.log");
        let log = fs::File::create(&log_path).expect("create the agent log");
        let mut child = self
            .command(&["agent", "--auto", "--idle-shutdown"])
            .stdout(Stdio::from(log.try_clone().expect("clone the agent log")))
            .stderr(Stdio::from(log))
            .spawn()
            .expect("the local agent starts");
        let started = Instant::now();
        let status: ExitStatus = loop {
            if let Some(status) = child.try_wait().expect("poll the local agent") {
                break status;
            }
            if started.elapsed() >= AGENT_DEADLINE {
                let _ = child.kill();
                let _ = child.wait();
                panic!(
                    "the local agent never drained the queue within {AGENT_DEADLINE:?}:\n{}",
                    fs::read_to_string(&log_path).unwrap_or_default()
                );
            }
            std::thread::sleep(Duration::from_millis(200));
        };
        let log = fs::read_to_string(&log_path).expect("read the agent log");
        assert!(status.success(), "the local agent exited badly:\n{log}");
        log
    }

    pub fn read(&self, relative: &str) -> String {
        let path = self.storage.join(relative);
        fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("read persisted {}: {error}", path.display()))
    }

    pub fn holds(&self, relative: &str) -> bool {
        self.storage.join(relative).exists()
    }

    pub fn record(&self, prefix: &str, job: &str) -> Value {
        serde_json::from_str(&self.read(&format!("{prefix}/{job}.json")))
            .expect("the persisted job record is JSON")
    }

    pub fn scratch(&self, name: &str) -> PathBuf {
        let path = self.root.path().join(name);
        fs::create_dir_all(&path).expect("create a journey scratch directory");
        path
    }

    /// Did the workload really run inside this journey's own job tree? macOS
    /// resolves the tempdir through `/private`, so both sides are canonical.
    pub fn is_own_workdir(&self, path: &str, job: &str) -> bool {
        let home = fs::canonicalize(&self.home).expect("canonicalize the journey home");
        fs::canonicalize(path).is_ok_and(|workdir| workdir.starts_with(home)) && path.ends_with(job)
    }
}

fn registry(hostname: &str) -> Value {
    json!({
        "schema_version": 2,
        "targets": [{
            "name": TARGET,
            "kind": "local",
            "ssh": null,
            "release_platform": platform(),
            "hostnames": [hostname.to_lowercase()],
            // This agent claims routed work only, so nothing but the job a
            // case submits can ever be started here.
            "pinned_only": true,
            // Admission fails closed until a host can read its own disk
            // policy, so the isolated registry states one. `off` keeps the
            // janitor from deleting anything.
            "disk_cleanup": {
                "mode": "off",
                "check_interval_seconds": 86400,
                "low_free_gb": 1,
                "target_free_gb": 2,
                "max_bytes_per_pass": 1048576,
                "max_items_per_pass": 1,
                "max_scan_items": 1,
                "cleaners": {},
            },
        }],
        "coordinators": [],
    })
}
