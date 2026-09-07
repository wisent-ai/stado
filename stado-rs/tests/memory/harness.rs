//! The isolated registry, store and binary invocation the memory tests share.

use std::fs;
use std::path::Path;
use std::process::{Command, Output};

use crate::constants::{
    ALWAYS_OVER_LOW_MB, ALWAYS_OVER_TARGET_MB, MAX_REPAIRS_PER_PASS, REGISTRY_SCHEMA_VERSION,
    UNREACHABLE_SWAP_PCT,
};

pub const TARGET: &str = "memory-fixture";

/// A launchd label and systemd unit name nothing on a developer machine
/// loads, so `restart_unit` reaches its declared subject and records that the
/// host reports no such unit instead of restarting something real.
pub const ABSENT_UNIT: &str = "com.wisent.memory-fixture-absent";

pub fn stado(storage: &Path, args: &[&str]) -> Output {
    let home = storage.join("home");
    fs::create_dir_all(&home).unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_stado"));
    command
        .args(args)
        .env("HOME", &home)
        .env("WC_STORAGE_BACKEND", "local")
        .env("WC_LOCAL_STORAGE_PATH", storage)
        .env("STADO_CONFIG", storage.join("no-such-config.json"))
        .env_remove("COMPUTE_API_KEY")
        .env_remove("COMPUTE_API_URL")
        .env_remove("WC_PROFILES_DIR");
    command.output().expect("stado binary runs")
}

pub fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

pub fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn system_hostname() -> String {
    let output = Command::new("hostname").output().unwrap();
    String::from_utf8(output.stdout)
        .unwrap()
        .trim()
        .to_ascii_lowercase()
}

fn release_platform() -> &'static str {
    if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        "darwin-arm64"
    } else if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
        "darwin-amd64"
    } else if cfg!(target_arch = "aarch64") {
        "linux-arm64"
    } else {
        "linux-amd64"
    }
}

/// A registry carrying one local target that declares nothing about memory.
pub fn setup() -> tempfile::TempDir {
    let storage = tempfile::tempdir().unwrap();
    let document = serde_json::json!({
        "schema_version": REGISTRY_SCHEMA_VERSION,
        "targets": [{
            "name": TARGET,
            "kind": "local",
            "release_platform": release_platform(),
            "hostnames": [system_hostname()],
        }],
        "coordinators": [],
    });
    fs::write(
        storage.path().join("registry.json"),
        serde_json::to_string_pretty(&document).unwrap(),
    )
    .unwrap();
    storage
}

pub fn registry_bytes(storage: &Path) -> String {
    fs::read_to_string(storage.join("registry.json")).unwrap()
}

/// The memory report the janitor pass writes, read out of `disk-cleanup`'s
/// own stdout: the disk report is printed first and the memory report second,
/// each one canonical JSON on a line of its own.
pub fn run_pass(storage: &Path) -> serde_json::Value {
    let output = stado(storage, &["disk-cleanup", "--once"]);
    assert!(
        output.status.success(),
        "disk-cleanup --once failed: {}",
        stderr(&output)
    );
    let text = stdout(&output);
    let last = text
        .lines()
        .rfind(|line| line.starts_with('{'))
        .unwrap_or_else(|| panic!("no JSON report on stdout: {text}"));
    serde_json::from_str(last).expect("the memory report is one JSON document")
}

/// Declare a watermark no machine can be under, plus whatever else the test
/// names.
pub fn declare(storage: &Path, extra: &[&str]) -> Output {
    let low = ALWAYS_OVER_LOW_MB.to_string();
    let target = ALWAYS_OVER_TARGET_MB.to_string();
    let swap = UNREACHABLE_SWAP_PCT.to_string();
    let repairs = MAX_REPAIRS_PER_PASS.to_string();
    let mut args = vec![
        "space",
        "watermark",
        TARGET,
        "--memory-low-free-mb",
        &low,
        "--memory-target-free-mb",
        &target,
        "--memory-high-swap-used-pct",
        &swap,
        "--memory-max-repairs-per-pass",
        &repairs,
    ];
    args.extend_from_slice(extra);
    stado(storage, &args)
}
