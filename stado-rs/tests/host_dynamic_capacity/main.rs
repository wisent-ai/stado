//! Real CLI journey for live host capacity without fixed worker counts.
//!
//! The journey publishes the same capacity document as the managed agent, then
//! drives the built `stado` binary through both the healthy and refused forms of
//! `host gates`. It uses an isolated local Stado store and never reads or writes
//! the operator's registry or configuration.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::{json, Value};

const HOST: &str = "dynamic-capacity-runner";

struct Journey {
    home: tempfile::TempDir,
    storage: PathBuf,
    hostname: String,
}

impl Journey {
    fn new() -> Self {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/host-dynamic-capacity-runs");
        fs::create_dir_all(&root).unwrap();
        let home = tempfile::Builder::new()
            .prefix("host-dynamic-capacity-")
            .tempdir_in(root)
            .unwrap();
        let storage = home.path().join("store");
        fs::create_dir_all(&storage).unwrap();
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
        let registry = json!({
            "schema_version": 2,
            "targets": [{
                "name": HOST,
                "kind": "local",
                "ssh": "nobody@127.0.0.1",
                "release_platform": platform,
                "hostnames": [hostname],
                "disk_cleanup": {
                    "mode": "off",
                    "check_interval_seconds": 300,
                    "low_free_gb": 1,
                    "target_free_gb": 2,
                    "max_bytes_per_pass": 1_073_741_824_u64,
                    "max_items_per_pass": 10,
                    "max_scan_items": 1_000,
                    "cleaners": {}
                }
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
            hostname,
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

    fn invoke(&self, args: &[&str]) -> Output {
        self.command().args(args).output().unwrap()
    }

    fn publish(&self, accepting_jobs: bool, available_cpu_cores: u64, queue_paused: bool) {
        let capacity = json!({
            "consumer_id": format!("local-{}", self.hostname),
            "kind": "local",
            "accepting_jobs": accepting_jobs,
            "running_jobs": 3,
            "total_cpu_cores": 12,
            "available_cpu_cores": available_cpu_cores,
            "available_accelerators": {"apple-m2-max": 1},
            "free_ram_gb": 18.5,
            "total_ram_gb": 64.0,
            "free_vram_gb": 7.5,
            "total_vram_gb": 32.0,
            "published_at": chrono::Utc::now().to_rfc3339(),
            "diag": {
                "disk_pressure_unresolved": false,
                "disk_cleanup_policy_known": true,
                "queue_paused": queue_paused,
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
}

fn text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

#[test]
#[ignore = "Probierz records the real host-capacity CLI journey"]
fn host_gates_use_live_resources_and_never_fixed_slots() {
    let journey = Journey::new();
    journey.publish(true, 6, false);

    let healthy = journey.invoke(&["host", "gates", HOST, "--json"]);
    assert!(
        healthy.status.success(),
        "healthy capacity failed\n{}",
        text(&healthy)
    );
    let report: Value = serde_json::from_slice(&healthy.stdout).unwrap();
    assert_eq!(report["claiming"], true);
    assert_eq!(report["capacity"]["accepting_jobs"], true);
    assert_eq!(report["capacity"]["running_jobs"], 3);
    assert_eq!(report["capacity"]["available_cpu_cores"], 6);
    assert_eq!(report["capacity"]["total_cpu_cores"], 12);
    assert_eq!(report["capacity"]["free_ram_gb"], 18.5);
    assert_eq!(report["capacity"]["total_ram_gb"], 64.0);
    assert_eq!(report["capacity"]["free_vram_gb"], 7.5);
    assert_eq!(report["capacity"]["total_vram_gb"], 32.0);
    assert_eq!(
        report["capacity"]["available_accelerators"]["apple-m2-max"],
        1
    );
    let capacity = report["capacity"].as_object().unwrap();
    for removed in ["slots", "free_slots", "max_concurrent_jobs"] {
        assert!(
            !capacity.contains_key(removed),
            "removed fixed-capacity field {removed} leaked into the public contract"
        );
    }

    let healthy_text = journey.invoke(&["host", "gates", HOST]);
    assert!(healthy_text.status.success(), "{}", text(&healthy_text));
    assert!(
        !text(&healthy_text).to_ascii_lowercase().contains("slot"),
        "the human contract retained fixed slot wording: {}",
        text(&healthy_text)
    );

    journey.publish(false, 0, true);
    let refused = journey.invoke(&["host", "gates", HOST, "--json"]);
    assert!(!refused.status.success(), "a paused host was accepted");
    let refused_report: Value = serde_json::from_slice(&refused.stdout).unwrap();
    assert_eq!(refused_report["claiming"], false);
    assert_eq!(refused_report["capacity"]["available_cpu_cores"], 0);
    assert_eq!(refused_report["blockers"], json!(["queue_paused"]));

    let refused_text = journey.invoke(&["host", "gates", HOST]);
    assert!(!refused_text.status.success(), "a paused host was accepted");
    assert!(
        text(&refused_text).contains(&format!("{HOST} is claiming nothing: queue_paused")),
        "the refusal did not name the host and blocker: {}",
        text(&refused_text)
    );
}
