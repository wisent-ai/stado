//! The isolated registry, the product invocation and the report reader shared
//! by the declared-repair cases.
//!
//! Split out of `main.rs` so each file stays inside the three hundred line
//! limit this repository enforces on itself.

use std::path::Path;
use std::process::{Command, Output};

use serde_json::{json, Value};

/// The service whose repair steps carry executable implementations.
pub const SERVICE: &str = "stado";
pub const TARGET: &str = "repair-observation-host";
pub const DECLARATION: &str = "stado-rs/data/service-catalog.json";

/// The release platform this machine really is, in the product's own spelling.
pub fn platform() -> &'static str {
    if std::env::consts::OS == "macos" {
        "darwin-arm64"
    } else {
        "linux-amd64"
    }
}

/// The other declared platform, for the case that checks a declaration this
/// machine contradicts.
pub fn other_platform() -> &'static str {
    if platform() == "darwin-arm64" {
        "linux-amd64"
    } else {
        "darwin-arm64"
    }
}

pub fn hostname() -> String {
    let output = Command::new("/bin/hostname")
        .output()
        .expect("the operating system reports its host name");
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

/// An isolated registry naming this machine, so the production code takes its
/// current-host path instead of reaching for a remote destination.
pub fn storage(declared_platform: &str) -> tempfile::TempDir {
    let directory = tempfile::tempdir().expect("an isolated storage root");
    let registry = json!({
        "schema_version": 2,
        "targets": [{
            "name": TARGET,
            "kind": "local",
            "ssh": null,
            "release_platform": declared_platform,
            "hostnames": [hostname()],
            "services": [],
        }],
        "coordinators": [],
    });
    std::fs::write(
        directory.path().join("registry.json"),
        serde_json::to_vec_pretty(&registry).expect("registry serialises"),
    )
    .expect("seed the isolated registry");
    directory
}

/// `HOME` sits inside the same tempdir as the store: the product records a
/// last-known-good registry copy under `HOME`, so a spawn that inherited the
/// operator's home would write the operator's own `~/.stado/cache`.
pub fn stado(storage: &Path, args: &[&str]) -> Output {
    let home = storage.join("home");
    std::fs::create_dir_all(&home).expect("an isolated home");
    Command::new(env!("CARGO_BIN_EXE_stado"))
        .args(args)
        .env("HOME", &home)
        .env("WC_STORAGE_BACKEND", "local")
        .env("WC_LOCAL_STORAGE_PATH", storage)
        .env("STADO_CONFIG", storage.join("no-such-config.json"))
        .env_remove("COMPUTE_API_KEY")
        .env_remove("COMPUTE_API_URL")
        .env_remove("WC_PROFILES_DIR")
        .output()
        .expect("the built stado binary runs")
}

pub fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

pub fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

pub fn report(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "stdout was not one JSON report: {error}\nstdout={}\nstderr={}",
            stdout(output),
            stderr(output)
        )
    })
}

pub fn first_observation(report: &Value) -> &Value {
    &report["steps"][0]["observation"]
}
