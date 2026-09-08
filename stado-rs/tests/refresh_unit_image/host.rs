//! The isolated current-host fixture every case in this area drives.
//!
//! One registry target, and it is this machine: the target's name is this
//! host's own name lower-cased, its `hostnames` carry the name
//! `providers::vast::system_hostname` returns, and it declares no SSH
//! destination at all. That is what makes `lookup_self` resolve and
//! `host_channel::target_is_this_host` true — the gate `restart_local_unit`
//! refuses on — so the command runs its current-host path and drives this
//! machine's own `/bin/launchctl`, `/usr/bin/plutil`, `/usr/sbin/lsof` and
//! `/bin/ps`. Nothing is substituted on PATH.
//!
//! Isolation is the fixture's other job: a fresh tempdir per case, `HOME` and
//! the local storage root inside it, and `STADO_CONFIG` pointing at a path
//! that does not exist — so the operator's own registry, vault, fleet and
//! launchd units are unreachable from every case here.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::Value;
use sha2::{Digest, Sha256};

/// The only PATH the product is given: this machine's own system
/// directories, so every tool it runs is the real one.
pub const SYSTEM_PATH: &str = "/usr/bin:/bin:/usr/sbin:/sbin";

/// The isolated host: its tempdir, and the registry naming this machine.
pub struct Host {
    _dir: tempfile::TempDir,
    pub root: PathBuf,
    pub home: PathBuf,
    pub storage: PathBuf,
    /// This machine's own host name, lower-cased. It is the registry target's
    /// name, so there is no stand-in host name anywhere in this area.
    pub target: String,
}

impl Host {
    /// A fixture declaring this machine and no service.
    pub fn new() -> Self {
        let dir = tempfile::Builder::new()
            .prefix("stado-refresh-image-")
            .tempdir()
            .expect("create the isolated host");
        let root = dir.path().to_path_buf();
        let host = Self {
            home: root.join("home"),
            storage: root.join("storage"),
            root,
            _dir: dir,
            target: local_host_name(),
        };
        for directory in [
            host.agents(),
            host.storage.clone(),
            host.root.join("bin"),
            host.root.join("tmp"),
        ] {
            std::fs::create_dir_all(&directory).expect("create an isolated directory");
        }
        host.declare();
        host
    }

    /// The launchd agent directory the product scans, inside this tempdir.
    pub fn agents(&self) -> PathBuf {
        self.home.join("Library/LaunchAgents")
    }

    /// Declare this machine as the registry's only target.
    pub fn declare(&self) {
        self.write(serde_json::json!({
            "schema_version": stado::targets::REGISTRY_SCHEMA_VERSION,
            "targets": [{
                "name": self.target,
                "kind": "local",
                "ssh": null,
                "release_platform": release_platform(),
                "hostnames": [hostname(), self.target],
                "role": "always-on",
                "host_heuristic": "always-on",
                "managed_versions": {},
                "services": [],
            }],
            "coordinators": [],
        }));
    }

    /// A registry that declares no target at all, which is the honest way to
    /// reach the refusal for a machine no target names: no invented host is
    /// needed to have none.
    pub fn declare_no_machine(&self) {
        self.write(serde_json::json!({
            "schema_version": stado::targets::REGISTRY_SCHEMA_VERSION,
            "targets": [],
            "coordinators": [],
        }));
    }

    fn write(&self, document: Value) {
        std::fs::write(
            self.storage.join("registry.json"),
            serde_json::to_string_pretty(&document).expect("registry document"),
        )
        .expect("write the fixture registry");
    }

    /// Run the built binary with nothing of the operator's environment left.
    pub fn stado(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_stado"))
            .args(args)
            .env_clear()
            .env("HOME", &self.home)
            .env("PATH", SYSTEM_PATH)
            .env("TMPDIR", self.root.join("tmp"))
            .env("STADO_CONFIG", self.root.join("no-such-config.json"))
            .env("WC_STORAGE_BACKEND", "local")
            .env("WC_LOCAL_STORAGE_PATH", &self.storage)
            .env("NO_COLOR", "1")
            .output()
            .expect("the built stado binary did not start")
    }

    /// `stado service refresh-image <label>`.
    pub fn refresh(&self, label: &str) -> Output {
        self.stado(&["service", "refresh-image", label])
    }
}

/// This machine's host name, by the call the product resolves its own target
/// with.
pub fn hostname() -> String {
    stado::providers::vast::system_hostname()
}

/// The registry target name: this machine's own host name, lower-cased and
/// without the mDNS suffix, which is the form a registry write accepts.
pub fn local_host_name() -> String {
    let name = hostname().to_lowercase();
    name.trim_end_matches(".local").to_string()
}

/// The sha256 of a file, computed here rather than taken from the product:
/// this is the independent measurement a verdict is checked against.
pub fn digest(path: &Path) -> String {
    let bytes = std::fs::read(path).expect("read the file to digest");
    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    hex::encode(hasher.finalize())
}

/// Everything the command wrote, so an assertion does not have to guess which
/// stream carried the refusal.
pub fn said(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn release_platform() -> &'static str {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => "darwin-arm64",
        ("macos", _) => "darwin-amd64",
        (_, "aarch64") => "linux-arm64",
        _ => "linux-amd64",
    }
}
