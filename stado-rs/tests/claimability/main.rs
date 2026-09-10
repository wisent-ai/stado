//! A queue with no claimant, and how the product says so.
//!
//! Every test drives the built `stado` binary (`CARGO_BIN_EXE_stado`) with
//! WC_STORAGE_BACKEND=local + WC_LOCAL_STORAGE_PATH=<TempDir>. STADO_CONFIG
//! points at a nonexistent path so the developer's real config can never leak
//! in, and the registry document, the queued jobs, the capacity publications
//! and the health beacons all live and die inside the temp dir. Nothing here
//! reads the fleet's store or touches a host.
//!
//! What is under test is `stado status` and `stado overview` as reports: a
//! stuck queue is stated, a moving one is not mentioned, and neither says
//! anything about the exit status — both stay 0, because this is a report and
//! not a gate.
//!
//! Every fixture is copied from the live incident it was written for, not
//! invented. Job `2c4a47aa` is `bash inputs/run.sh`, submitted
//! 2026-08-14T19:11:37Z, `provider: local`, pinned to
//! `local-control-host.local`, and queued for 121 hours. The vocabulary
//! is the vocabulary `stado host gates control-host --json` printed on
//! that day: `blockers: ["no_capacity_publication", "pinned_only"]` with
//! `capacity.published_at: null`. The mini's queue-agent declaration is its
//! real one, `com.wisent.compute.service.stado-agent-mini` at
//! `/Users/charles/Library/LaunchAgents/...`, a unit its own health beacon
//! does not report.

use std::path::Path;
use std::process::{Command, Output};

use serde_json::Value;

/// The real job's id, command and submitter, so a sentence about `2c4a47aa`
/// in a test is a sentence about the job the operator stared at.
const JOB_ID: &str = "2c4a47aa";
/// 121 hours and 38 minutes, the wait the live store held. Kept as an offset
/// from the test's own clock so the sentence is the same on any day.
const WAITED_SECONDS: i64 = 121 * 3600 + 38 * 60;
/// The mini's declared queue agent, verbatim from the registry.
const AGENT_LABEL: &str = "com.wisent.compute.service.stado-agent-mini";
const AGENT_PLIST: &str =
    "/Users/charles/Library/LaunchAgents/com.wisent.compute.service.stado-agent-mini.plist";

/// Three `kind=local` hosts in the shape the live registry declares them: a
/// pinned Mac mini that declares its queue agent as a user LaunchAgent, a
/// pinned Linux box that declares no agent at all, and an unpinned laptop.
const REGISTRY: &str = r#"{
    "schema_version": 2,
    "targets": [
        {
            "name": "mini",
            "kind": "local",
            "ssh": "charles@10.0.0.253",
            "release_platform": "darwin-arm64",
            "hostnames": ["mini.local"],
            "pinned_only": true,
            "services": [
                {
                    "kind": "launchd",
                    "name": "com.wisent.compute.service.stado-agent-mini",
                    "label": "com.wisent.compute.service.stado-agent-mini",
                    "path": "/Users/charles/Library/LaunchAgents/com.wisent.compute.service.stado-agent-mini.plist",
                    "unit": "",
                    "managed_since": "2026-08-19T00:46:51.797832+00:00"
                }
            ]
        },
        {
            "name": "rtx",
            "kind": "local",
            "ssh": "root@10.0.0.108",
            "release_platform": "linux-amd64",
            "hostnames": ["rtx-box"],
            "gpu_type": "nvidia-rtx-pro-6000",
            "pinned_only": true
        },
        {
            "name": "laptop",
            "kind": "local",
            "ssh": "op@10.0.0.234",
            "release_platform": "darwin-arm64",
            "hostnames": ["laptop.local"]
        }
    ],
    "coordinators": []
}"#;

fn stado(storage: &Path, args: &[&str]) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_stado"));
    cmd.args(args)
        .env("WC_STORAGE_BACKEND", "local")
        .env("WC_LOCAL_STORAGE_PATH", storage)
        // A set-but-missing STADO_CONFIG disables config-file discovery.
        .env("STADO_CONFIG", storage.join("no-such-config.json"))
        .env_remove("COMPUTE_API_KEY")
        .env_remove("COMPUTE_API_URL")
        .env_remove("WC_PROFILES_DIR");
    cmd.output().expect("stado binary runs")
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// A temp store carrying [`REGISTRY`] and nothing else.
fn fleet() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("registry.json"), REGISTRY).unwrap();
    dir
}

fn write(storage: &Path, name: &str, body: &Value) {
    let path = storage.join(name);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, serde_json::to_string(body).unwrap()).unwrap();
}

/// `seconds` before the moment this test runs, RFC-3339.
fn ago(seconds: i64) -> String {
    (chrono::Utc::now() - chrono::Duration::seconds(seconds)).to_rfc3339()
}

/// The live queued job: `provider: local`, cpu, and pinned to `pinned_host`.
fn queue_job(storage: &Path, job_id: &str, waited_seconds: i64, pinned_host: &str) {
    write(
        storage,
        &format!("queue/{job_id}.json"),
        &serde_json::json!({
            "job_id": job_id,
            "command": "bash inputs/run.sh",
            "provider": "local",
            "state": "queued",
            "created_at": ago(waited_seconds),
            "pinned_host": pinned_host,
            "assigned_to": pinned_host,
            "submitted_by": "lukaszbartoszcze",
            "submitted_from": "laptop.local",
        }),
    );
}

/// One capacity broadcast in the shape `queue::capacity::publish_capacity`
/// writes it.
fn publish(storage: &Path, consumer_id: &str, age_seconds: i64, diag: Value) {
    write(
        storage,
        &format!("capacity/{consumer_id}.json"),
        &serde_json::json!({
            "consumer_id": consumer_id,
            "kind": consumer_id.split_once('-').map(|(kind, _)| kind).unwrap_or("local"),
            "free_slots": {"cpu": 1},
            "free_vram_gb": 1,
            "total_vram_gb": 1,
            "published_at": ago(age_seconds),
            "diag": diag,
        }),
    );
}

/// A health beacon reporting exactly `units`.
fn beacon(storage: &Path, slug: &str, units: Value) {
    write(
        storage,
        &format!("host_health/{slug}.json"),
        &serde_json::json!({
            "host": slug,
            "reported_at": ago(30),
            "units": units,
        }),
    );
}

/// The `claimability` section of `stado overview --json`.
fn claimability(storage: &Path) -> Value {
    let out = stado(storage, &["overview", "--json"]);
    assert!(out.status.success(), "overview --json exits 0");
    let document: Value = serde_json::from_str(&stdout(&out)).expect("overview --json is JSON");
    document["claimability"].clone()
}

/// A queue nobody publishes capacity for is named as such, host by host, in
/// each host's own words — and the report is still a report: exit 0.
mod pins;
mod verdicts;
