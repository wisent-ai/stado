//! The isolated fleet the reservation stories run against: this machine as
//! one local target, the real agent publishing into a local store, and a
//! real `stado capacity hold` taking a reservation on it.

use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

pub(crate) const TARGET: &str = "reservation-runner";

/// The registry contract the product's own writer stamps.
const REGISTRY_SCHEMA_VERSION: i64 = 2;
/// A disk-cleanup declaration the validator accepts and the janitor never
/// acts on.
const DISK_CHECK_INTERVAL_SECONDS: i64 = 300;
const DISK_LOW_FREE_GB: i64 = 1;
const DISK_TARGET_FREE_GB: i64 = 2;
const DISK_MAX_BYTES_PER_PASS: u64 = 1_073_741_824;
const DISK_MAX_ITEMS_PER_PASS: i64 = 10;
const DISK_MAX_SCAN_ITEMS: i64 = 100;
/// A publication in the agent's shape for a host with no core to give.
const FULL_HOST_TOTAL_CPU_CORES: i64 = 8;
const FULL_HOST_AVAILABLE_CPU_CORES: i64 = 0;
const FULL_HOST_FREE_RAM_GB: f64 = 20.0;
const FULL_HOST_TOTAL_RAM_GB: f64 = 32.0;
/// A reservation whose holder stopped answering long ago: the smallest
/// declared kind, the default TTL, heartbeat two hours back.
const DEAD_RESERVATION_SCHEMA_VERSION: i64 = 1;
const DEAD_RESERVATION_CPU_CORES: i64 = 1;
const DEAD_RESERVATION_RAM_GB: f64 = 1.0;
const DEAD_RESERVATION_TTL_SECONDS: i64 = 180;
pub(crate) const LONG_EXPIRED: Duration = Duration::from_secs(2 * 3600);

pub(crate) struct Journey {
    /// Kept after the run: every command's output, the agent's log, the store.
    pub(crate) home: PathBuf,
    pub(crate) storage: PathBuf,
    pub(crate) agent: Option<Child>,
}

impl Journey {
    pub(crate) fn new() -> Self {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/capacity-reservation-runs");
        fs::create_dir_all(&root).unwrap();
        let home = tempfile::Builder::new()
            .prefix("reservation-")
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
        let bin = home.join(".stado/bin");
        fs::create_dir_all(&bin).unwrap();
        std::os::unix::fs::symlink(env!("CARGO_BIN_EXE_stado"), bin.join("stado")).unwrap();
        let hostname =
            String::from_utf8(Command::new("hostname").arg("-f").output().unwrap().stdout)
                .unwrap()
                .trim()
                .to_ascii_lowercase();
        let registry = json!({
            "schema_version": REGISTRY_SCHEMA_VERSION,
            "targets": [{
                "name": TARGET,
                "kind": "local",
                "ssh": "nobody@127.0.0.1",
                "hostnames": [hostname],
                "release_platform": build_platform(),
                "disk_cleanup": {
                    "mode": "off",
                    "check_interval_seconds": DISK_CHECK_INTERVAL_SECONDS,
                    "low_free_gb": DISK_LOW_FREE_GB,
                    "target_free_gb": DISK_TARGET_FREE_GB,
                    "max_bytes_per_pass": DISK_MAX_BYTES_PER_PASS,
                    "max_items_per_pass": DISK_MAX_ITEMS_PER_PASS,
                    "max_scan_items": DISK_MAX_SCAN_ITEMS,
                    "cleaners": {}
                }
            }],
            "coordinators": []
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

    /// The consumer id this machine's agent publishes under.
    pub(crate) fn consumer_id(&self) -> String {
        let hostname =
            String::from_utf8(Command::new("hostname").arg("-f").output().unwrap().stdout)
                .unwrap()
                .trim()
                .to_ascii_lowercase();
        format!("local-{hostname}")
    }

    /// Publish this host as full: accepting, but with no core to give.
    pub(crate) fn publish_full_host(&self) {
        let consumer = self.consumer_id();
        let directory = self.storage.join("capacity");
        fs::create_dir_all(&directory).unwrap();
        fs::write(
            directory.join(format!("{consumer}.json")),
            serde_json::to_string_pretty(&json!({
                "consumer_id": consumer,
                "kind": "local",
                "accepting_jobs": true,
                "running_jobs": 0,
                "running_workloads": 0,
                "total_cpu_cores": FULL_HOST_TOTAL_CPU_CORES,
                "available_cpu_cores": FULL_HOST_AVAILABLE_CPU_CORES,
                "available_accelerators": {},
                "free_ram_gb": FULL_HOST_FREE_RAM_GB,
                "total_ram_gb": FULL_HOST_TOTAL_RAM_GB,
                "free_vram_gb": 0,
                "total_vram_gb": 0,
                "reserved": {"cpu_cores": 0, "ram_gb": 0.0, "vram_gb": 0},
                "reservations": [],
                "published_at": chrono::Utc::now().to_rfc3339(),
                "diag": {},
                "stado_version": env!("CARGO_PKG_VERSION")
            }))
            .unwrap(),
        )
        .unwrap();
    }

    /// Seed one reservation in the product's own shape whose heartbeat is
    /// two hours old, and return its path.
    pub(crate) fn seed_dead_reservation(&self, kind: &str) -> PathBuf {
        let consumer = self.consumer_id();
        let directory = self.storage.join("state/reservations").join(&consumer);
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join("dead.json");
        let long_ago = chrono::Utc::now() - chrono::Duration::from_std(LONG_EXPIRED).unwrap();
        fs::write(
            &path,
            serde_json::to_string(&json!({
                "schema_version": DEAD_RESERVATION_SCHEMA_VERSION,
                "reservation_id": "dead",
                "consumer_id": consumer,
                "target": TARGET,
                "kind": kind,
                "product": "weles-worker",
                "holder": "a process that is gone",
                "cpu_cores": DEAD_RESERVATION_CPU_CORES,
                "ram_gb": DEAD_RESERVATION_RAM_GB,
                "vram_gb": 0,
                "acquired_at": long_ago.to_rfc3339(),
                "heartbeat_at": long_ago.to_rfc3339(),
                "ttl_seconds": DEAD_RESERVATION_TTL_SECONDS
            }))
            .unwrap(),
        )
        .unwrap();
        File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(std::time::SystemTime::now() - LONG_EXPIRED)
            .unwrap();
        path
    }

    pub(crate) fn command(&self) -> Command {
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

    pub(crate) fn invoke(&self, args: &[&str]) -> Output {
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

    pub(crate) fn start_agent(&mut self) {
        let stdout = File::create(self.home.join("agent.out")).unwrap();
        let stderr = File::create(self.home.join("agent.err")).unwrap();
        self.agent = Some(
            self.command()
                .args(["agent", "--target", TARGET])
                .stdout(Stdio::from(stdout))
                .stderr(Stdio::from(stderr))
                .spawn()
                .unwrap(),
        );
    }

    /// Start a real `stado capacity hold` and return the child; its stdout
    /// is the JSON receipt, retained beside the run.
    pub(crate) fn start_hold(&self, kind: &str, seconds: u64) -> Child {
        let stdout = File::create(self.home.join("hold.out")).unwrap();
        let stderr = File::create(self.home.join("hold.err")).unwrap();
        self.command()
            .args([
                "capacity",
                "hold",
                "--kind",
                kind,
                "--target",
                TARGET,
                "--seconds",
                &seconds.to_string(),
                "--json",
            ])
            .stdout(Stdio::from(stdout))
            .stderr(Stdio::from(stderr))
            .spawn()
            .unwrap()
    }

    pub(crate) fn wait_for(
        &mut self,
        description: &str,
        timeout: Duration,
        predicate: impl Fn(&Self) -> bool,
    ) {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if predicate(self) {
                return;
            }
            if let Some(agent) = self.agent.as_mut() {
                if let Some(status) = agent.try_wait().unwrap() {
                    panic!(
                        "agent exited before {description}: {status}\nstdout:\n{}\nstderr:\n{}",
                        fs::read_to_string(self.home.join("agent.out")).unwrap_or_default(),
                        fs::read_to_string(self.home.join("agent.err")).unwrap_or_default(),
                    );
                }
            }
            thread::sleep(Duration::from_millis(100));
        }
        panic!(
            "timed out waiting for {description}\nstdout:\n{}\nstderr:\n{}",
            fs::read_to_string(self.home.join("agent.out")).unwrap_or_default(),
            fs::read_to_string(self.home.join("agent.err")).unwrap_or_default(),
        );
    }

    pub(crate) fn newest_capacity(&self) -> Option<Value> {
        let directory = self.storage.join("capacity");
        let entry = fs::read_dir(directory)
            .ok()?
            .flatten()
            .find(|entry| entry.path().extension().is_some_and(|ext| ext == "json"))?;
        serde_json::from_slice(&fs::read(entry.path()).ok()?).ok()
    }

    /// Every reservation document under `state/reservations/`, any consumer.
    pub(crate) fn reservation_files(&self) -> Vec<PathBuf> {
        let root = self.storage.join("state/reservations");
        let mut files = Vec::new();
        let Ok(consumers) = fs::read_dir(&root) else {
            return files;
        };
        for consumer in consumers.flatten() {
            if let Ok(entries) = fs::read_dir(consumer.path()) {
                files.extend(
                    entries
                        .flatten()
                        .map(|entry| entry.path())
                        .filter(|path| path.extension().is_some_and(|ext| ext == "json")),
                );
            }
        }
        files.sort();
        files
    }

    pub(crate) fn unmet_files(&self) -> Vec<PathBuf> {
        let root = self.storage.join("state/fleet/unmet");
        let mut files = Vec::new();
        let Ok(days) = fs::read_dir(&root) else {
            return files;
        };
        for day in days.flatten() {
            if let Ok(entries) = fs::read_dir(day.path()) {
                files.extend(entries.flatten().map(|entry| entry.path()));
            }
        }
        files.sort();
        files
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

pub(crate) fn build_platform() -> &'static str {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => "darwin-arm64",
        ("linux", "x86_64") => "linux-amd64",
        (os, arch) => panic!("reservation journey has no platform mapping for {os}-{arch}"),
    }
}
