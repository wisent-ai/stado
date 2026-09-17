//! The isolated fleet the needs stories read: a registry each story shapes,
//! a capacity publication in the managed agent's own shape, and the real
//! binary driven with every command's output retained.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::{json, Value};

pub(crate) const HOST: &str = "needs-host";
pub(crate) const LINUX_ONLY: &str = "needs-linux";

/// The registry contract the product's own writer stamps.
const REGISTRY_SCHEMA_VERSION: i64 = 2;
/// The declared watermarks the stories measure the host against: memory
/// low 2 GiB / target 4 GiB / swap 80 %, disk low 8 GiB / target 20 GiB.
const MEMORY_CHECK_INTERVAL_SECONDS: i64 = 300;
const MEMORY_LOW_FREE_MB: i64 = 2048;
const MEMORY_TARGET_FREE_MB: i64 = 4096;
const MEMORY_HIGH_SWAP_PCT: i64 = 80;
const MEMORY_MAX_REPAIRS: i64 = 1;
const DISK_CHECK_INTERVAL_SECONDS: i64 = 300;
const DISK_LOW_FREE_GB: i64 = 8;
const DISK_TARGET_FREE_GB: i64 = 20;
const DISK_MAX_BYTES_PER_PASS: u64 = 1_073_741_824;
const DISK_MAX_ITEMS_PER_PASS: i64 = 10;
const DISK_MAX_SCAN_ITEMS: i64 = 100;
/// One host under pressure: a 16 GiB machine with 1.5 GiB available, swap
/// at 90 %, and 6 GiB of disk free.
const PRESSED_TOTAL_CPU_CORES: i64 = 10;
const PRESSED_AVAILABLE_CPU_CORES: i64 = 7;
const PRESSED_TOTAL_RAM_GB: f64 = 16.0;
const PRESSED_AVAILABLE_RAM_GB: f64 = 1.5;
const PRESSED_SWAP_USED_PCT: f64 = 90.0;
const PRESSED_FREE_DISK_GB: f64 = 6.0;
/// The pressed host runs nothing and has no GPU.
const NOTHING: i64 = 0;

pub(crate) struct Journey {
    pub(crate) home: PathBuf,
    pub(crate) storage: PathBuf,
    hostname: String,
}

impl Journey {
    /// A fleet with this machine as `HOST` (platform of this build) and one
    /// declared Linux host nobody can reach.
    pub(crate) fn new() -> Self {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/fleet-needs-runs");
        fs::create_dir_all(&root).unwrap();
        let home = tempfile::Builder::new()
            .prefix("fleet-needs-")
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
        let platform = if cfg!(target_os = "macos") {
            "darwin-arm64"
        } else {
            "linux-amd64"
        };
        let disk_cleanup = |mode: &str| {
            json!({
                "mode": mode,
                "check_interval_seconds": DISK_CHECK_INTERVAL_SECONDS,
                "low_free_gb": DISK_LOW_FREE_GB,
                "target_free_gb": DISK_TARGET_FREE_GB,
                "max_bytes_per_pass": DISK_MAX_BYTES_PER_PASS,
                "max_items_per_pass": DISK_MAX_ITEMS_PER_PASS,
                "max_scan_items": DISK_MAX_SCAN_ITEMS,
                "cleaners": {}
            })
        };
        let registry = json!({
            "schema_version": REGISTRY_SCHEMA_VERSION,
            "targets": [
                {
                    "name": HOST,
                    "kind": "local",
                    "ssh": "nobody@127.0.0.1",
                    "release_platform": platform,
                    "hostnames": [hostname],
                    "memory_reclaim": {
                        "mode": "report",
                        "check_interval_seconds": MEMORY_CHECK_INTERVAL_SECONDS,
                        "low_free_mb": MEMORY_LOW_FREE_MB,
                        "target_free_mb": MEMORY_TARGET_FREE_MB,
                        "high_swap_used_pct": MEMORY_HIGH_SWAP_PCT,
                        "max_repairs_per_pass": MEMORY_MAX_REPAIRS,
                        "refuse_placement": false,
                        "repairs": {}
                    },
                    "disk_cleanup": disk_cleanup("report")
                },
                {
                    "name": LINUX_ONLY,
                    "kind": "local",
                    "ssh": "nobody@127.0.0.2",
                    "release_platform": "linux-amd64",
                    "hostnames": ["needs-linux.invalid"],
                    "disk_cleanup": disk_cleanup("off")
                }
            ],
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
            hostname,
        }
    }

    /// Drop `HOST` from the registry, leaving only the Linux host: a fleet
    /// with no Mac.
    pub(crate) fn without_this_host(&self) {
        let path = self.storage.join("registry.json");
        let mut registry: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        let targets = registry["targets"].as_array_mut().unwrap();
        targets.retain(|target| target["name"] == LINUX_ONLY);
        fs::write(&path, serde_json::to_string_pretty(&registry).unwrap()).unwrap();
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

    /// Publish `HOST` under memory pressure and below its disk target, in
    /// the shape the managed agent publishes.
    pub(crate) fn publish_pressed_host(&self) {
        let consumer = format!("local-{}", self.hostname);
        let directory = self.storage.join("capacity");
        fs::create_dir_all(&directory).unwrap();
        fs::write(
            directory.join(format!("{consumer}.json")),
            serde_json::to_string_pretty(&json!({
                "consumer_id": consumer,
                "kind": "local",
                "accepting_jobs": true,
                "running_jobs": NOTHING,
                "running_workloads": NOTHING,
                "total_cpu_cores": PRESSED_TOTAL_CPU_CORES,
                "available_cpu_cores": PRESSED_AVAILABLE_CPU_CORES,
                "available_accelerators": {},
                "free_ram_gb": PRESSED_AVAILABLE_RAM_GB,
                "total_ram_gb": PRESSED_TOTAL_RAM_GB,
                "free_vram_gb": NOTHING,
                "total_vram_gb": NOTHING,
                "published_at": chrono::Utc::now().to_rfc3339(),
                "diag": {
                    "free_disk_gb": PRESSED_FREE_DISK_GB,
                    "memory_pressure_active": true,
                    "memory_available_gb": PRESSED_AVAILABLE_RAM_GB,
                    "memory_total_gb": PRESSED_TOTAL_RAM_GB,
                    "memory_low_watermark_gb": (MEMORY_LOW_FREE_MB as f64) / 1024.0,
                    "memory_swap_used_pct": PRESSED_SWAP_USED_PCT,
                    "memory_swap_high_watermark_pct": MEMORY_HIGH_SWAP_PCT
                },
                "stado_version": env!("CARGO_PKG_VERSION")
            }))
            .unwrap(),
        )
        .unwrap();
    }
}
