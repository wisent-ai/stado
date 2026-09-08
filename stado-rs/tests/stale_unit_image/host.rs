//! The isolated current-host fixture every case in this area drives.
//!
//! One registry target, and it is this machine: the target's name is this
//! host's own name lower-cased, its `hostnames` carry the name
//! `providers::vast::system_hostname` returns, and it declares no SSH
//! destination at all. That is what makes `lookup_self` resolve and
//! `host_channel::target_is_this_host` true, so the product runs its
//! current-host path and asks this machine's own `/bin/launchctl`,
//! `/usr/bin/plutil`, `/usr/sbin/lsof` and `/bin/ps`. Nothing is substituted
//! on PATH.
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

/// `registry doctor`'s two row kinds this area reads
/// (`StaleUnitImage::kind`).
pub const STALE: &str = "stale-unit-image";
pub const UNREAD: &str = "unread-unit-image";

/// The isolated host: its tempdir, and the registry naming this machine.
pub struct Host {
    _dir: tempfile::TempDir,
    pub root: PathBuf,
    pub home: PathBuf,
    pub storage: PathBuf,
    /// This machine's own host name, lower-cased. It is the registry target's
    /// name, so it is also the `subject` of every finding — there is no
    /// stand-in host name anywhere in this area.
    pub target: String,
}

impl Host {
    /// A fixture declaring this machine and no service.
    pub fn new() -> Self {
        let dir = tempfile::Builder::new()
            .prefix("stado-stale-image-")
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
        host.declare(&[]);
        host
    }

    /// The launchd agent directory the product scans, inside this tempdir.
    pub fn agents(&self) -> PathBuf {
        self.home.join("Library/LaunchAgents")
    }

    /// Declare this machine, with `services` as its adopted units.
    pub fn declare(&self, services: &[Value]) {
        let document = serde_json::json!({
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
                "services": services,
            }],
            "coordinators": [],
        });
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

    /// Every `registry doctor` finding of KIND, as JSON.
    pub fn findings(&self, kind: &str) -> Vec<Value> {
        let output = self.stado(&["registry", "doctor", "--json"]);
        let document: Value = serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
            panic!(
                "registry doctor printed no JSON document ({error}):\n{}{}",
                said(&output.stdout),
                said(&output.stderr)
            )
        });
        document["findings"]
            .as_array()
            .map(|rows| {
                rows.iter()
                    .filter(|row| row["finding"].as_str() == Some(kind))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The rows of KIND that name `label`.
    ///
    /// Filtered by label rather than counted: the machine running this test is
    /// a real host with real launchd directories, and a fleet unit that
    /// genuinely is stale here is a true positive, not this area's business.
    pub fn about(&self, kind: &str, label: &str) -> Vec<Value> {
        self.findings(kind)
            .into_iter()
            .filter(|row| row["detail"].as_str().is_some_and(|it| it.contains(label)))
            .collect()
    }

    /// The one row of KIND naming `label`, with everything the doctor said
    /// when there is not exactly one.
    pub fn only_row(&self, kind: &str, label: &str) -> Value {
        let rows = self.about(kind, label);
        assert_eq!(
            rows.len(),
            1,
            "expected exactly one {kind} row for {label}, got {rows:#?}"
        );
        rows.into_iter().next().expect("one row")
    }
}

/// The `services[]` element `stado service adopt` writes.
pub fn adopted(label: &str, path: &Path) -> Value {
    serde_json::json!({
        "name": label,
        "unit": "",
        "label": label,
        "path": path.display().to_string(),
        "kind": "launchd",
        "managed_since": chrono::Utc::now().to_rfc3339(),
    })
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

/// One stream as text, so a failure prints what was actually said.
pub fn said(stream: &[u8]) -> String {
    String::from_utf8_lossy(stream).to_string()
}

fn release_platform() -> &'static str {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => "darwin-arm64",
        ("macos", _) => "darwin-amd64",
        (_, "aarch64") => "linux-arm64",
        _ => "linux-amd64",
    }
}
