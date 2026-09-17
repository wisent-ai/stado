//! The isolated fleet the gates stories run against: two local targets, one
//! of them this machine, a coordinator entry so `coordinator --once` runs,
//! and the real binary driven with every command's output retained.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::{json, Value};

pub const HOST: &str = "gates-fixture";
pub const ELSEWHERE: &str = "gates-elsewhere";

/// The registry contract the product's own writer stamps
/// (`targets::REGISTRY_SCHEMA_VERSION`).
const REGISTRY_SCHEMA_VERSION: i64 = 2;
/// A disk-cleanup declaration the validator accepts and the janitor never
/// acts on: `mode: off`, the smallest legal watermarks, one small pass.
const DISK_CHECK_INTERVAL_SECONDS: i64 = 300;
const DISK_LOW_FREE_GB: i64 = 1;
const DISK_TARGET_FREE_GB: i64 = 2;
const DISK_MAX_BYTES_PER_PASS: u64 = 1_073_741_824;
const DISK_MAX_ITEMS_PER_PASS: i64 = 10;
const DISK_MAX_SCAN_ITEMS: i64 = 1_000;
/// The coordinator cadence declared for the fixture; `--once` ignores it.
const COORDINATOR_INTERVAL_SECONDS: i64 = 60;
/// One healthy capacity reading, in the shape the managed agent publishes.
const CAPACITY_RUNNING_JOBS: i64 = 0;
const CAPACITY_TOTAL_CPU_CORES: i64 = 12;
const CAPACITY_AVAILABLE_CPU_CORES: i64 = 8;
const CAPACITY_FREE_RAM_GB: f64 = 24.0;
const CAPACITY_TOTAL_RAM_GB: f64 = 64.0;

pub struct Journey {
    /// Kept after the run: revision, every command's output, the store.
    pub home: PathBuf,
    pub storage: PathBuf,
    hostname: String,
    /// Counts submissions so every run id is unique within one journey.
    submissions: std::cell::Cell<u32>,
}

impl Journey {
    pub fn new() -> Self {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/host-gates-runs");
        fs::create_dir_all(&root).unwrap();
        let home = tempfile::Builder::new()
            .prefix("host-gates-")
            .tempdir_in(root)
            .unwrap()
            .keep();
        let storage = home.join("store");
        fs::create_dir_all(&storage).unwrap();
        let revision = Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(env!("CARGO_MANIFEST_DIR"))
            .output()
            .unwrap();
        fs::write(home.join("revision.txt"), revision.stdout).unwrap();
        let hostname =
            String::from_utf8(Command::new("hostname").arg("-f").output().unwrap().stdout)
                .unwrap()
                .trim()
                .to_ascii_lowercase();
        assert!(!hostname.is_empty(), "the journey host has no hostname");
        let platform = if cfg!(target_os = "macos") {
            "darwin-arm64"
        } else {
            "linux-amd64"
        };
        let disk_cleanup = json!({
            "mode": "off",
            "check_interval_seconds": DISK_CHECK_INTERVAL_SECONDS,
            "low_free_gb": DISK_LOW_FREE_GB,
            "target_free_gb": DISK_TARGET_FREE_GB,
            "max_bytes_per_pass": DISK_MAX_BYTES_PER_PASS,
            "max_items_per_pass": DISK_MAX_ITEMS_PER_PASS,
            "max_scan_items": DISK_MAX_SCAN_ITEMS,
            "cleaners": {}
        });
        let registry = json!({
            "schema_version": REGISTRY_SCHEMA_VERSION,
            "targets": [
                {
                    "name": HOST,
                    "kind": "local",
                    "ssh": "nobody@127.0.0.1",
                    "release_platform": platform,
                    "hostnames": [hostname],
                    "disk_cleanup": disk_cleanup,
                },
                {
                    "name": ELSEWHERE,
                    "kind": "local",
                    "ssh": "nobody@127.0.0.2",
                    "release_platform": platform,
                    "hostnames": ["gates-elsewhere.invalid"],
                    "disk_cleanup": disk_cleanup,
                }
            ],
            "coordinators": [{
                "name": "gates-coordinator",
                "runtime": "cron",
                "interval_seconds": COORDINATOR_INTERVAL_SECONDS,
                "active": true
            }]
        });
        fs::write(
            storage.join("registry.json"),
            serde_json::to_string_pretty(&registry).unwrap(),
        )
        .unwrap();
        // The gates read asks the host which store its agent writes to, and
        // answers that by running the installed product binary: install it.
        let bin = home.join(".stado/bin");
        fs::create_dir_all(&bin).unwrap();
        std::os::unix::fs::symlink(env!("CARGO_BIN_EXE_stado"), bin.join("stado")).unwrap();
        Self {
            home,
            storage,
            hostname,
            submissions: std::cell::Cell::new(0),
        }
    }

    /// Publish the capacity document the managed agent would publish for
    /// this host, so the verdict has a publication to decide from. The
    /// numbers are one healthy reading; the shape is the agent's.
    pub fn publish_capacity(&self) {
        let capacity = json!({
            "consumer_id": format!("local-{}", self.hostname),
            "kind": "local",
            "accepting_jobs": true,
            "running_jobs": CAPACITY_RUNNING_JOBS,
            "total_cpu_cores": CAPACITY_TOTAL_CPU_CORES,
            "available_cpu_cores": CAPACITY_AVAILABLE_CPU_CORES,
            "available_accelerators": {},
            "free_ram_gb": CAPACITY_FREE_RAM_GB,
            "total_ram_gb": CAPACITY_TOTAL_RAM_GB,
            "free_vram_gb": 0,
            "total_vram_gb": 0,
            "published_at": chrono::Utc::now().to_rfc3339(),
            "diag": {
                "disk_pressure_unresolved": false,
                "disk_cleanup_policy_known": true,
                "queue_paused": false,
                "pinned_only": false
            },
            "stado_version": env!("CARGO_PKG_VERSION")
        });
        let directory = self.storage.join("capacity");
        fs::create_dir_all(&directory).unwrap();
        fs::write(
            directory.join(format!("local-{}.json", self.hostname)),
            serde_json::to_string_pretty(&capacity).unwrap(),
        )
        .unwrap();
    }

    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_stado"));
        command
            .env_clear()
            .env("HOME", &self.home)
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("STADO_CONFIG", self.home.join("no-config.json"))
            .env("WC_STORAGE_BACKEND", "local")
            .env("WC_LOCAL_STORAGE_PATH", &self.storage)
            .env("WC_PROVIDERS", "local")
            .env("WC_VAST_AUTO_LIST", "false");
        command
    }

    pub fn invoke(&self, args: &[&str]) -> Output {
        let output = self.command().args(args).output().unwrap();
        let name = args.join("_").replace('/', "_");
        let evidence = self.home.join("evidence");
        fs::create_dir_all(&evidence).unwrap();
        fs::write(evidence.join(format!("{name}.stdout")), &output.stdout).unwrap();
        fs::write(evidence.join(format!("{name}.stderr")), &output.stderr).unwrap();
        fs::write(
            evidence.join(format!("{name}.exit")),
            format!("{:?}", output.status.code()),
        )
        .unwrap();
        output
    }

    pub fn invoke_ok(&self, args: &[&str]) -> Output {
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

    /// Submit one queued job pinned to `host` and return its id, read back
    /// from the queue document the product wrote.
    pub fn submit_pinned(&self, host: &str, command: &str) -> String {
        let sequence = self.submissions.get() + 1;
        self.submissions.set(sequence);
        let run_id = format!("gates-{host}-{sequence}");
        let output = self.invoke_ok(&[
            "submit",
            "--run-id",
            &run_id,
            "--provider",
            "local",
            "--pinned-host",
            host,
            command,
        ]);
        let text = String::from_utf8_lossy(&output.stdout);
        let id = text
            .split_whitespace()
            .find(|word| word.starts_with("job-"))
            .unwrap_or_else(|| panic!("submit printed no job id: {text}"))
            .trim_end_matches(|c: char| !c.is_ascii_alphanumeric())
            .to_string();
        assert!(
            self.storage.join("queue").join(format!("{id}.json")).exists(),
            "submit did not write queue/{id}.json"
        );
        id
    }

    pub fn queue_objects(&self) -> Vec<PathBuf> {
        let mut names: Vec<PathBuf> = fs::read_dir(self.storage.join("queue"))
            .unwrap()
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
            .collect();
        names.sort();
        names
    }

    pub fn queue_state(&self, id: &str) -> Option<String> {
        let path = self.storage.join("queue").join(format!("{id}.json"));
        let body = fs::read(path).ok()?;
        let document: Value = serde_json::from_slice(&body).ok()?;
        document["state"].as_str().map(str::to_string)
    }

    pub fn gates(&self) -> (Value, Output) {
        let output = self.invoke(&["host", "gates", HOST, "--json"]);
        let report = serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
            panic!(
                "gates printed no JSON report: {error}\nstdout={}\nstderr={}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            )
        });
        (report, output)
    }
}

pub fn observation<'a>(report: &'a Value, operation: &str) -> &'a Value {
    report["observations"]
        .as_array()
        .unwrap_or_else(|| panic!("the report lists its reads: {report}"))
        .iter()
        .find(|read| read["operation"] == operation)
        .unwrap_or_else(|| panic!("no {operation} read in {report}"))
}
