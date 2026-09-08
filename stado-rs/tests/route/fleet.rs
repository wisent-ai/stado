//! One isolated fleet per case: a fresh tempdir, local storage, a config path
//! that does not exist, and a registry whose single target names THIS machine.
//!
//! Naming this machine is what makes the journey real. `host_channel` matches
//! the registry entry's hostnames against the running kernel's, so every
//! `stado route ...` operation below takes the production current-host path
//! and executes the operating system's own tools against the tempdir — the
//! same shape `tests/host_exec/main.rs` uses. The `ssh` destination is present
//! because the service-directory validator requires the authority target to
//! declare one; it is never dialled, and the vault path every capability case
//! reads back proves the local branch ran.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::{json, Value};

pub const TARGET: &str = "route-real-host";
pub const SERVICE: &str = "route-real-service";
/// Nothing listens here. Every assertion is about the declared endpoint
/// travelling from the registry into the marker, never about reaching it.
pub const PORT: u64 = 48231;
const SYSTEM_PATH: &str = "/usr/bin:/bin:/usr/sbin:/sbin";

fn hostname() -> String {
    let output = Command::new("hostname")
        .env_clear()
        .env("PATH", SYSTEM_PATH)
        .output()
        .expect("blocked: the real hostname executable could not start");
    assert!(output.status.success(), "the real hostname read failed");
    let name = String::from_utf8(output.stdout)
        .expect("the kernel hostname is UTF-8")
        .trim()
        .to_ascii_lowercase();
    assert!(!name.is_empty(), "the real current host has no hostname");
    name
}

fn platform() -> String {
    format!(
        "{}-{}",
        match std::env::consts::OS {
            "macos" => "darwin",
            other => other,
        },
        match std::env::consts::ARCH {
            "aarch64" => "arm64",
            other => other,
        }
    )
}

pub struct Fleet {
    pub root: tempfile::TempDir,
    pub home: PathBuf,
    pub storage: PathBuf,
}

impl Fleet {
    pub fn new() -> Self {
        let root = tempfile::Builder::new()
            .prefix("stado-route-real-")
            .tempdir()
            .expect("an isolated journey root");
        let home = root.path().join("home");
        let storage = root.path().join("storage");
        for directory in [&home, &storage] {
            fs::create_dir_all(directory).expect("an isolated journey directory");
        }
        let fleet = Self {
            root,
            home,
            storage,
        };
        fleet.write_registry(&json!({
            "schema_version": crate::REGISTRY_SCHEMA_VERSION,
            "targets": [{
                "name": TARGET,
                "kind": "local",
                "ssh": "nobody@127.0.0.1",
                "release_platform": platform(),
                "hostnames": [hostname()],
                "services": [],
            }],
            "coordinators": [],
            "service_directory": {
                "authority": {"target": TARGET, "command": env!("CARGO_BIN_EXE_stado")},
                "generation": crate::FIRST_GENERATION,
                "services": {},
            },
        }));
        fleet
    }

    pub fn write_registry(&self, document: &Value) {
        fs::write(
            self.registry_path(),
            format!("{}\n", serde_json::to_string_pretty(document).unwrap()),
        )
        .expect("seed the isolated registry");
    }

    pub fn registry_path(&self) -> PathBuf {
        self.storage.join("registry.json")
    }

    /// The canonical document as it is persisted right now.
    pub fn registry(&self) -> Value {
        serde_json::from_str(&fs::read_to_string(self.registry_path()).unwrap())
            .expect("the persisted registry is JSON")
    }

    pub fn stado(&self, args: &[&str]) -> Output {
        self.stado_with(args, &[])
    }

    /// The built binary, with nothing of the operator's environment reachable:
    /// no real storage, no real config, no inherited credential variables.
    pub fn stado_with(&self, args: &[&str], extra: &[(&str, &Path)]) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_stado"));
        command
            .args(args)
            .env_clear()
            .env("HOME", &self.home)
            .env("PATH", SYSTEM_PATH)
            .env("WC_STORAGE_BACKEND", "local")
            .env("WC_LOCAL_STORAGE_PATH", &self.storage)
            .env("STADO_CONFIG", self.root.path().join("no-such-config.json"));
        for (key, value) in extra {
            command.env(key, value);
        }
        command.output().expect("the built stado binary runs")
    }

    /// Where `route open --local` writes the declared endpoint.
    pub fn marker(&self, service: &str) -> PathBuf {
        self.home
            .join(".stado/forwards")
            .join(format!("{service}.local"))
    }

    /// A declaration file for `stado service declare`. Without `endpoint` it
    /// is the declaration that names no resource for its own host.
    pub fn declaration(&self, name: &str, endpoint: bool) -> PathBuf {
        let mut document = json!({
            "name": name,
            "host": TARGET,
            "source": {
                "artifact": format!("{name}/1.0.0/{name}.tar.gz"),
                "sha256": "0".repeat(crate::SHA256_HEX_LEN),
            },
            "run": {"program": "/usr/bin/true", "args": ["--serve"]},
            "consumers": {"stado-route-tests": {"capabilities": ["read"]}},
        });
        if endpoint {
            document["port"] = json!(PORT);
        }
        let path = self.root.path().join(format!("{name}.declaration.json"));
        fs::write(&path, serde_json::to_string_pretty(&document).unwrap()).unwrap();
        path
    }
}

/// Both streams, because a refusal reaches stderr and a report reaches stdout,
/// and a case that read only one would miss the other entirely.
pub fn said(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

pub fn json_stdout(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|error| panic!("stado did not answer with JSON: {error}\n{}", said(output)))
}
