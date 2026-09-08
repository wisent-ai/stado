//! The isolated registry, the product invocation and the readers shared by the
//! host release-state cases.
//!
//! Split out of `main.rs` so each file stays inside the three hundred line
//! limit this repository enforces on itself.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::{json, Value};

pub const TARGET: &str = "host-release-observation";
pub const BINARY: &str = "stado";
/// Older than anything this fleet has shipped, so the host is ahead of it.
pub const STALE_VERSION: &str = "0.0.1";

pub fn platform() -> &'static str {
    if std::env::consts::OS == "macos" {
        "darwin-arm64"
    } else {
        "linux-amd64"
    }
}

/// The host name the registry requires: the kernel's own answer, normalized
/// the way the product validates it. A registry write is refused outright when
/// a declared host name is not normalized, which is why this is lower-cased
/// here rather than passed through.
pub fn hostname() -> String {
    let output = Command::new("/bin/hostname")
        .output()
        .expect("the operating system reports its host name");
    String::from_utf8_lossy(&output.stdout)
        .trim()
        .to_lowercase()
}

/// The managed binary this machine really carries, and the version it prints.
///
/// The product reads the same file, so this is the fact the report is checked
/// against. A machine without it is a real state too: the report has to say
/// the binary is not installed.
pub fn installed_binary() -> PathBuf {
    let home = std::env::var("HOME").expect("the test process has a home");
    Path::new(&home).join(".stado/bin").join(BINARY)
}

pub fn installed_version() -> Option<String> {
    let path = installed_binary();
    if !path.is_file() {
        return None;
    }
    let output = Command::new(&path).arg("--version").output().ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8_lossy(&output.stdout)
        .split_whitespace()
        .nth(usize::from(true))
        .map(str::to_string)
}

pub struct Fixture {
    root: tempfile::TempDir,
}

impl Fixture {
    /// An isolated registry naming this machine, so the release code takes its
    /// current-host path instead of reaching for a remote destination.
    pub fn new() -> Self {
        let root = tempfile::tempdir().expect("an isolated storage root");
        let registry = json!({
            "schema_version": 2,
            "targets": [{
                "name": TARGET,
                "kind": "local",
                "ssh": null,
                "release_platform": platform(),
                "hostnames": [hostname()],
                "services": [],
            }],
            "coordinators": [],
        });
        std::fs::write(
            root.path().join("registry.json"),
            serde_json::to_vec_pretty(&registry).expect("registry serialises"),
        )
        .expect("seed the isolated registry");
        Self { root }
    }

    pub fn path(&self) -> &Path {
        self.root.path()
    }

    pub fn stado(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_stado"))
            .args(args)
            .env("WC_STORAGE_BACKEND", "local")
            .env("WC_LOCAL_STORAGE_PATH", self.path())
            .env("STADO_CONFIG", self.path().join("no-such-config.json"))
            .env_remove("COMPUTE_API_KEY")
            .env_remove("COMPUTE_API_URL")
            .env_remove("STADO_API_URL")
            .env_remove("WC_PROFILES_DIR")
            .output()
            .expect("the built stado binary runs")
    }

    pub fn declare(&self, version: &str) -> Output {
        self.stado(&[
            "release",
            "declare-version",
            "--host",
            TARGET,
            "--binary",
            BINARY,
            "--version",
            version,
            "--json",
        ])
    }

    pub fn unset(&self) -> Output {
        self.stado(&[
            "release",
            "declare-version",
            "--host",
            TARGET,
            "--binary",
            BINARY,
            "--unset",
            "--json",
        ])
    }

    pub fn host_state(&self, extra: &[&str]) -> Output {
        let mut args = vec!["release", "host-state", "--host", TARGET, "--json"];
        args.extend_from_slice(extra);
        self.stado(&args)
    }

    /// The registry document as it stands on disk, which is where a declaration
    /// has to land to mean anything.
    pub fn registry(&self) -> Value {
        let text = std::fs::read_to_string(self.path().join("registry.json"))
            .expect("read the isolated registry");
        serde_json::from_str(&text).expect("the registry stays valid JSON")
    }

    pub fn declared_version(&self) -> Option<String> {
        self.registry()["targets"][0]["managed_versions"][BINARY]
            .as_str()
            .map(str::to_string)
    }
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

/// The one reported binary, so a case reads a fact rather than an index.
pub fn reported_binary(report: &Value) -> &Value {
    let binaries = report["binaries"]
        .as_array()
        .expect("the report carries the binaries it examined");
    assert_eq!(
        binaries.len(),
        usize::from(true),
        "exactly one declared binary was expected: {report}"
    );
    &binaries[0]
}
