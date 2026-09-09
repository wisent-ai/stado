//! The isolated registry, the delivery trees and the report reader shared by
//! the managed-version cases.
//!
//! The subject is this machine. The registry declares one target whose name
//! is the kernel's own host name, normalized the way the product validates
//! one, with this build's real release platform and no remote destination, so
//! `registry doctor` judges a host that exists. Every program a unit is
//! declared to run is a real file this fixture creates under a temporary
//! home, laid out in the `$HOME/.stado/services/<product>/current/<platform>`
//! shape the delivery tree really uses, so the product's path-shape reader is
//! reading a tree that is on this disk.
//!
//! Split out of `main.rs` so each file stays inside the three hundred line
//! limit this repository enforces on itself.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::{json, Value};

pub const VERSION_FINDING: &str = "undeclared-service-version";

/// The compiled `host release` catalog product every case below is measured
/// against, and the one this fleet delivers everywhere.
pub const CATALOG_PRODUCT: &str = "stado";

/// A catalog product that release control also owns, which is the whole
/// reason the two authorities must not both be required.
pub const CONTROLLED_PRODUCT: &str = "skarbiec";

pub const LEGACY: &str = "com.wisent.always-on.skarbiec-gateway";
pub const MANAGED: &str = "com.wisent.always-on.stado";
pub const LABEL_STAGED: &str = "com.wisent.always-on.object-api";
pub const ARBITRARY: &str = "com.wisent.always-on.weles-admission";

/// The release platform this machine really is, in the product's own spelling.
pub fn platform() -> &'static str {
    if std::env::consts::OS == "macos" {
        "darwin-arm64"
    } else {
        "linux-amd64"
    }
}

/// This machine's host name, normalized the way the registry validates one.
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
    host: String,
}

impl Fixture {
    pub fn new() -> Self {
        let root = tempfile::tempdir().expect("an isolated storage root");
        for sub in ["storage/host_health", "home"] {
            std::fs::create_dir_all(root.path().join(sub)).expect("temporary subdirectory");
        }
        Self {
            root,
            host: hostname(),
        }
    }

    pub fn host(&self) -> &str {
        &self.host
    }

    pub fn root(&self) -> &Path {
        self.root.path()
    }

    pub fn home(&self) -> PathBuf {
        self.root().join("home")
    }

    /// Create the program a delivery-tree unit runs, at the coordinate the
    /// tree really puts it: `$HOME/.stado/services/<tree>/current/<platform>`
    /// plus the executable's own relative path.
    ///
    /// The file is real so that the declaration a case makes is a declaration
    /// about something on this disk rather than about a string.
    pub fn delivery_program(&self, tree: &str, relative: &str) -> String {
        let path = self
            .home()
            .join(".stado/services")
            .join(tree)
            .join("current")
            .join(platform())
            .join(relative);
        std::fs::create_dir_all(path.parent().expect("a parent directory"))
            .expect("create the delivery tree");
        std::fs::write(&path, "#!/bin/sh\nexit 0\n").expect("create the delivered program");
        path.display().to_string()
    }

    /// One local target naming this machine.
    ///
    /// The name is the host's own identity and `hostnames` is therefore
    /// empty: this build refuses a document that declares one identity
    /// twice. No `always-on` role either, because this machine has a
    /// graphical login and a unit under its own `Library/LaunchAgents` is
    /// exactly where such a host loads one.
    pub fn declare(&self, services: &[Value], versions: Value, release_control: Option<Value>) {
        let mut document = json!({
            "schema_version": 2,
            "targets": [{
                "name": self.host,
                "kind": "local",
                "ssh": null,
                "release_platform": platform(),
                "hostnames": [],
                "managed_versions": versions,
                "services": services,
            }],
            "coordinators": [],
        });
        if let Some(release_control) = release_control {
            document["release_control"] = release_control;
        }
        std::fs::write(
            self.root().join("storage/registry.json"),
            serde_json::to_vec_pretty(&document).expect("registry serialises"),
        )
        .expect("seed the isolated registry");
    }

    pub fn beacon(&self, active: &[&str]) {
        let units: serde_json::Map<String, Value> = active
            .iter()
            .map(|unit| ((*unit).to_string(), json!({"state": "active"})))
            .collect();
        let beacon = json!({
            "host": self.host,
            "reported_at": chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string(),
            "units": units,
        });
        std::fs::write(
            self.root()
                .join("storage/host_health")
                .join(format!("{}.json", self.host)),
            serde_json::to_vec(&beacon).expect("beacon serialises"),
        )
        .expect("seed the beacon where the product reads it");
    }

    pub fn stado(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_stado"))
            .args(args)
            .env_clear()
            .env("HOME", self.home())
            .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
            .env("WC_STORAGE_BACKEND", "local")
            .env("WC_LOCAL_STORAGE_PATH", self.root().join("storage"))
            .env("STADO_CONFIG", self.root().join("storage/no-such.json"))
            .output()
            .expect("the built stado binary runs")
    }

    pub fn findings(&self, kind: &str) -> Vec<Value> {
        let output = self.stado(&["registry", "doctor", "--json"]);
        let document: Value = serde_json::from_slice(&output.stdout)
            .unwrap_or_else(|error| panic!("doctor emitted no JSON ({error}): {output:?}"));
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

    pub fn details(&self, kind: &str) -> Vec<String> {
        self.findings(kind)
            .iter()
            .map(|row| row["detail"].as_str().unwrap_or_default().to_string())
            .collect()
    }

    pub fn only(&self, kind: &str) -> Value {
        let rows = self.findings(kind);
        assert_eq!(rows.len(), 1, "expected exactly one {kind} row: {rows:?}");
        rows.into_iter().next().expect("one row")
    }
}

/// A `services[]` element as `stado service adopt|deploy` writes one, with the
/// unit file under this fixture's own home.
pub fn unit(home: &Path, label: &str, program: &str) -> Value {
    json!({
        "name": label,
        "unit": "",
        "label": label,
        "path": home
            .join("Library/LaunchAgents")
            .join(format!("{label}.plist"))
            .display()
            .to_string(),
        "kind": "launchd",
        "program": program,
        "args": [],
        "managed_since": "2026-08-19T00:46:51.797832+00:00",
    })
}

/// A `release_control` block owning CONTROLLED_PRODUCT on this host, with the
/// legacy launchd unit its rollout boots out.
pub fn release_control(host: &str, home: &Path) -> Value {
    let home = home.display().to_string();
    json!({
        "schema_version": 1,
        "generation": 4,
        "trusted_keys": {},
        "products": {
            CONTROLLED_PRODUCT: {
                "service": CONTROLLED_PRODUCT,
                "config_schema": 1,
                "state_schema": 1,
                "install_root": format!("{home}/.stado/services/{CONTROLLED_PRODUCT}"),
                "binary": "bin/skarbiec",
                "launcher": "bin/start-with-vault",
                "binary_env": "SKARBIEC_BIN",
                "port_env": "SKARBIEC_PORT_OVERRIDE",
                "runtime_env": "SKARBIEC_RUNTIME_DIR",
                "strategy": {
                    "kind": "blue-green",
                    "readiness_timeout_seconds": 90,
                    "drain_timeout_seconds": 60,
                    "rollback_window_seconds": 300,
                    "automatic_rollback": true
                },
                "targets": {
                    host: {
                        "platform": platform(),
                        "run_as_user": "stado",
                        "home": home,
                        "state_dir": format!("{home}/.stado/release-state"),
                        "runtime_root": format!("{home}/.stado/run"),
                        "logs_root": format!("{home}/.stado/logs"),
                        "stable_bind": "127.0.0.1:8080",
                        "candidate_ports": [18080, 18081],
                        "readiness_path": "/health",
                        "legacy_launchd_label": LEGACY,
                        "legacy_launchd_plist":
                            format!("{home}/Library/LaunchAgents/{LEGACY}.plist")
                    }
                }
            }
        }
    })
}
