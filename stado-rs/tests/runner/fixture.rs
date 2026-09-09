//! The isolated registry, the product invocation and the readers shared by the
//! runner observation cases.
//!
//! Split out of `main.rs` so each file stays inside the three hundred line
//! limit this repository enforces on itself.

use std::path::Path;
use std::process::{Command, Output};

use serde_json::{json, Value};

pub const TARGET: &str = "runner-observation-host";
pub const PROFILE_DECLARATION: &str = "stado-rs/data/runner-profiles.json";

pub fn platform() -> &'static str {
    if std::env::consts::OS == "macos" {
        "darwin-arm64"
    } else {
        "linux-amd64"
    }
}

/// The host name the registry requires: the kernel's answer, normalized the way
/// the product validates it.
pub fn hostname() -> String {
    let output = Command::new("/bin/hostname")
        .output()
        .expect("the operating system reports its host name");
    String::from_utf8_lossy(&output.stdout)
        .trim()
        .to_lowercase()
}

pub struct Fixture {
    root: tempfile::TempDir,
}

impl Fixture {
    /// An isolated registry naming this machine, so every runner read takes the
    /// current-host path and executes this machine's own service tooling.
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

    /// Every case runs through here, so the `HOME` override belongs here too:
    /// the product records its last-known-good registry copy under `HOME`, and
    /// a spawn inheriting the operator's home writes the operator's own
    /// `~/.stado/cache`.
    pub fn stado(&self, args: &[&str]) -> Output {
        let home = self.root.path().join("home");
        std::fs::create_dir_all(&home).expect("an isolated home");
        Command::new(env!("CARGO_BIN_EXE_stado"))
            .args(args)
            .env("HOME", &home)
            .env("WC_STORAGE_BACKEND", "local")
            .env("WC_LOCAL_STORAGE_PATH", self.root.path())
            .env("STADO_CONFIG", self.root.path().join("no-such-config.json"))
            .env_remove("COMPUTE_API_KEY")
            .env_remove("COMPUTE_API_URL")
            .env_remove("WC_PROFILES_DIR")
            .output()
            .expect("the built stado binary runs")
    }

    pub fn declared_profiles(&self) -> Vec<Value> {
        let output = self.stado(&["runner", "list", "--json"]);
        assert!(output.status.success(), "{}", stderr(&output));
        report(&output)["profiles"]
            .as_array()
            .expect("the build declares runner profiles")
            .clone()
    }

    pub fn status(&self, extra: &[&str]) -> Output {
        let mut args = vec!["runner", "status", TARGET, "--json"];
        args.extend_from_slice(extra);
        self.stado(&args)
    }

    pub fn diagnostics(&self, profile: &str) -> Output {
        self.stado(&[
            "runner",
            "diagnostics",
            "--profile",
            profile,
            TARGET,
            "--json",
        ])
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

/// Whether a runner is really configured at `root`: the agent writes `.runner`
/// when it registers, so this is the fact an `installed` claim is checked
/// against.
pub fn runner_is_configured(root: &str) -> bool {
    !root.is_empty() && Path::new(root).join(".runner").is_file()
}
