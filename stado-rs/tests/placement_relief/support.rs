//! The isolated fleet each placement relief story runs against: a registry
//! declaring one placement profile on two hosts, in the live registry's
//! shape, and the capacity publications the host agents would write, all
//! inside a temp dir the real `stado` binary reads as its local store.

use std::path::Path;
use std::process::{Command, Output};

use serde_json::Value;

pub(crate) const MINI: &str = "mini";
pub(crate) const LAPTOP: &str = "laptop";
pub(crate) const PROFILE: &str = "brama-skarbiec";
/// A Linux workstation in the registry that the profile does not declare.
pub(crate) const RTX: &str = "rtx";

/// The mini's live memory reading on 2026-09-18, the incident this stage was
/// written after: total, available, and swap in use, as its agent published
/// them.
pub(crate) const MINI_TOTAL_GB: f64 = 16.0;
pub(crate) const MINI_AVAILABLE_GB: f64 = 1.4;
pub(crate) const MINI_SWAP_PCT: f64 = 71.0;
/// The laptop declared in the same profile, with most of its memory free.
pub(crate) const LAPTOP_TOTAL_GB: f64 = 64.0;
pub(crate) const LAPTOP_AVAILABLE_GB: f64 = 40.0;

/// A publication age comfortably inside the fleet's capacity staleness
/// horizon, and one comfortably past it.
pub(crate) const FRESH_SECONDS: i64 = 30;
pub(crate) const STALE_SECONDS: i64 = 3 * 3600;

/// The registry document schema the live registry carries.
const REGISTRY_SCHEMA_VERSION: u16 = 2;
/// The slot and VRAM figures every capacity broadcast fixture in this
/// repository carries, so the publication parses as the agent writes it.
const FIXTURE_CPU_SLOTS: u16 = 1;
/// The placement relief report schema the stage writes, so a previous
/// report a story plants is read back as the stage's own.
pub(crate) const RELIEF_SCHEMA_VERSION: u16 = 1;
const FIXTURE_VRAM_GB: u16 = 1;

fn unit(host_label: &str, path: &str) -> Value {
    serde_json::json!({
        "kind": "launchd",
        "name": host_label,
        "label": host_label,
        "path": path,
        "unit": "",
        "managed_since": "2026-08-25T21:17:42.113148+00:00",
        "program": "/usr/bin/true",
        "args": []
    })
}

fn placement_unit(host_label: &str, path: &str) -> Value {
    serde_json::json!({
        "kind": "launchd",
        "name": host_label,
        "path": path,
        "unit": host_label
    })
}

/// Two `kind=local` hosts that both declare the profile's two units, so the
/// profile is complete on whichever host the directory places it on.
pub(crate) fn registry(active_host: &str) -> Value {
    let mini_brama = "com.wisent.always-on.brama";
    let mini_skarbiec = "com.wisent.always-on.skarbiec";
    let laptop_brama = "com.wisent.compute.service.com.wisent.brama";
    let laptop_skarbiec = "com.wisent.skarbiec";
    let mini_brama_path = "/Library/LaunchDaemons/com.wisent.always-on.brama.plist";
    let mini_skarbiec_path = "/Library/LaunchDaemons/com.wisent.always-on.skarbiec.plist";
    let laptop_brama_path =
        "/Users/op/Library/LaunchAgents/com.wisent.compute.service.com.wisent.brama.plist";
    let laptop_skarbiec_path = "/Users/op/Library/LaunchAgents/com.wisent.skarbiec.plist";
    let (mini_services, laptop_services) = if active_host == MINI {
        (
            vec![
                unit(mini_brama, mini_brama_path),
                unit(mini_skarbiec, mini_skarbiec_path),
            ],
            Vec::new(),
        )
    } else {
        (
            Vec::new(),
            vec![
                unit(laptop_brama, laptop_brama_path),
                unit(laptop_skarbiec, laptop_skarbiec_path),
            ],
        )
    };
    serde_json::json!({
        "schema_version": REGISTRY_SCHEMA_VERSION,
        "targets": [
            {
                "name": MINI,
                "kind": "local",
                "ssh": "charles@10.0.0.253",
                "release_platform": "darwin-arm64",
                "hostnames": ["mini.local"],
                "services": mini_services
            },
            {
                "name": LAPTOP,
                "kind": "local",
                "ssh": "op@10.0.0.234",
                "release_platform": "darwin-arm64",
                "hostnames": ["laptop.local"],
                "services": laptop_services
            },
            {
                "name": RTX,
                "kind": "local",
                "ssh": "root@10.0.0.108",
                "release_platform": "linux-amd64",
                "hostnames": ["rtx-box"]
            }
        ],
        "coordinators": [],
        "placement_profiles": [
            {
                "name": PROFILE,
                "services": ["brama", "skarbiec"],
                "start_order": ["skarbiec", "brama"],
                "stop_order": ["brama", "skarbiec"],
                "state": [{"path": ".stado/skarbiec.vault.json", "required": true}],
                "routing": [],
                "hosts": {
                    MINI: {
                        "probes": [],
                        "units": {
                            "brama": placement_unit(mini_brama, mini_brama_path),
                            "skarbiec": placement_unit(mini_skarbiec, mini_skarbiec_path)
                        }
                    },
                    LAPTOP: {
                        "probes": [],
                        "units": {
                            "brama": placement_unit(laptop_brama, laptop_brama_path),
                            "skarbiec": placement_unit(laptop_skarbiec, laptop_skarbiec_path)
                        }
                    }
                }
            }
        ]
    })
}

pub(crate) fn stado(storage: &Path, args: &[&str]) -> Output {
    let home = storage.join("home");
    std::fs::create_dir_all(&home).unwrap();
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_stado"));
    cmd.args(args)
        .env("HOME", &home)
        .env("WC_STORAGE_BACKEND", "local")
        .env("WC_LOCAL_STORAGE_PATH", storage)
        .env("STADO_CONFIG", storage.join("no-such-config.json"))
        .env_remove("COMPUTE_API_KEY")
        .env_remove("COMPUTE_API_URL")
        .env_remove("WC_PROFILES_DIR");
    cmd.output().expect("stado binary runs")
}

pub(crate) fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

pub(crate) fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

/// A temp store carrying the registry with the profile placed on
/// `active_host`, and nothing else.
pub(crate) fn fleet(active_host: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "registry.json", &registry(active_host));
    dir
}

pub(crate) fn write(storage: &Path, name: &str, body: &Value) {
    let path = storage.join(name);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, serde_json::to_string(body).unwrap()).unwrap();
}

/// `seconds` before the moment this test runs, RFC-3339.
pub(crate) fn ago(seconds: i64) -> String {
    (chrono::Utc::now() - chrono::Duration::seconds(seconds)).to_rfc3339()
}

/// One host's memory, in the flat fields the host memory policy publishes
/// beside its admission verdict.
pub(crate) fn memory(available_gb: f64, total_gb: f64, swap_pct: f64, pressure: bool) -> Value {
    let mut diag = serde_json::json!({
        "memory_available_gb": available_gb,
        "memory_total_gb": total_gb,
        "memory_swap_used_pct": swap_pct,
        "memory_refuse_placement": true,
    });
    if pressure {
        diag["memory_pressure_active"] = Value::Bool(true);
        diag["admission_reason"] = Value::String("memory_pressure_active".to_string());
    }
    diag
}

/// One capacity broadcast in the shape `queue::capacity::publish_capacity`
/// writes it, for the agent of `host`.
pub(crate) fn publish(storage: &Path, host: &str, age_seconds: i64, diag: Value) {
    let consumer_id = format!("local-{host}.local");
    write(
        storage,
        &format!("capacity/{consumer_id}.json"),
        &serde_json::json!({
            "consumer_id": consumer_id,
            "kind": "local",
            "free_slots": {"cpu": FIXTURE_CPU_SLOTS},
            "free_vram_gb": FIXTURE_VRAM_GB,
            "total_vram_gb": FIXTURE_VRAM_GB,
            "published_at": ago(age_seconds),
            "diag": diag,
        }),
    );
}

/// `stado placement relief --json`, parsed.
pub(crate) fn relief(storage: &Path) -> Value {
    let out = stado(storage, &["placement", "relief", "--json"]);
    assert!(
        out.status.success(),
        "placement relief failed: {}\n{}",
        stderr(&out),
        stdout(&out)
    );
    serde_json::from_str(&stdout(&out)).expect("placement relief prints one JSON document")
}

/// The one row for [`PROFILE`].
pub(crate) fn row(report: &Value) -> Value {
    let rows = report["rows"].as_array().expect("rows");
    rows.iter()
        .find(|row| row["profile"] == PROFILE)
        .cloned()
        .unwrap_or_else(|| panic!("no row for {PROFILE}: {report}"))
}

/// The verdict the row recorded for one candidate host.
pub(crate) fn verdict(row: &Value, host: &str) -> String {
    row["candidates"]
        .as_array()
        .expect("candidates")
        .iter()
        .find(|candidate| candidate["host"] == host)
        .and_then(|candidate| candidate["verdict"].as_str())
        .map(str::to_string)
        .unwrap_or_else(|| panic!("no candidate {host} in {row}"))
}
