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
/// The one cleaner `space report` requires a target to declare; one day.
const BUILD_CACHE_MIN_AGE_SECONDS: i64 = 86_400;
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
                    "cleaners": {"build_caches": {"min_age_seconds": BUILD_CACHE_MIN_AGE_SECONDS}}
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

mod drive;
mod read;
mod seed;
