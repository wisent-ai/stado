//! The isolated registry, the unit files and the report reader shared by the
//! unreachable-product-environment cases.
//!
//! The subject is this machine. The registry declares one target whose name
//! is the kernel's own host name normalized the way the product validates it,
//! so `registry doctor` resolves its current host to that target and takes
//! its local path: it opens the unit files this fixture writes, on this disk,
//! instead of reaching for a destination that does not exist. Every path in
//! the document is therefore a path a case can create and remove, which is
//! what makes a verdict about a missing program checkable.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::{json, Value};

use crate::document::platform;

/// This machine's host name, normalized the way the registry validates one.
///
/// A registry write is refused outright when a declared host name is not
/// normalized, which is why this is lower-cased here rather than passed
/// through, and it is the same answer `registry doctor` resolves its own
/// target with.
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
        for sub in ["storage/host_health", "home/Library/LaunchAgents"] {
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

    /// Where this machine keeps a per-user launchd unit, inside the fixture.
    pub fn plist_path(&self, label: &str) -> PathBuf {
        self.home()
            .join("Library/LaunchAgents")
            .join(format!("{label}.plist"))
    }

    /// Write a launchd unit for LABEL and return the path it now occupies.
    ///
    /// An empty `env` writes the empty `EnvironmentVariables` dict the
    /// incident's hand-created plist held.
    pub fn write_plist(&self, label: &str, program: &str, env: &[(&str, &str)]) -> PathBuf {
        let entries = env
            .iter()
            .map(|(name, value)| format!("    <key>{name}</key><string>{value}</string>"))
            .collect::<Vec<String>>()
            .join("\n");
        let body = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>{label}</string>
  <key>EnvironmentVariables</key>
  <dict>
{entries}
  </dict>
  <key>ProgramArguments</key>
  <array><string>{program}</string></array>
</dict>
</plist>
"#
        );
        let path = self.plist_path(label);
        std::fs::write(&path, body).expect("write the unit file this host will be read for");
        path
    }

    /// Take the unit file away again, leaving the record pointing at nothing.
    pub fn remove_plist(&self, label: &str) {
        std::fs::remove_file(self.plist_path(label)).expect("remove the unit file");
    }

    /// One local target naming this machine, with the services,
    /// `managed_versions` and `release_control` block a case declares.
    ///
    /// The name is the host's own identity and `hostnames` is therefore
    /// empty: this build refuses a document that declares one identity twice,
    /// and a refused document is a finding of its own that would sit on top
    /// of every case here. No `always-on` role either, because this machine
    /// has a graphical login and a unit under its own `Library/LaunchAgents`
    /// is exactly where such a host loads one.
    pub fn declare(&self, services: &[Value], versions: Value, release_control: Value) {
        let document = json!({
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
            "release_control": release_control,
        });
        std::fs::write(
            self.root().join("storage/registry.json"),
            serde_json::to_vec_pretty(&document).expect("registry serialises"),
        )
        .expect("seed the isolated registry");
    }

    /// A fresh beacon reporting every declared unit active, so no liveness
    /// finding can be mistaken for one of these.
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

    /// Every finding of KIND that `registry doctor` reports, as JSON rows.
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

    /// The detail sentences of every finding of KIND.
    pub fn details(&self, kind: &str) -> Vec<String> {
        self.findings(kind)
            .iter()
            .map(|row| row["detail"].as_str().unwrap_or_default().to_string())
            .collect()
    }

    /// The one finding of KIND, with its own sentence in the panic message.
    pub fn only(&self, kind: &str) -> Value {
        let rows = self.findings(kind);
        assert_eq!(rows.len(), 1, "expected exactly one {kind} row: {rows:?}");
        rows.into_iter().next().expect("one row")
    }
}
