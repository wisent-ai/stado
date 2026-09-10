//! The isolated fleet these stories run against, the readers they use, and
//! the real loopback listener that stands in for a serving process.

use std::net::TcpListener;
use std::process::{Command, Output};

/// The label every test addresses, declared on the target itself.
pub(crate) const SERVICE: &str = "com.wisent.always-on.weles";

pub(crate) struct Fleet {
    home: tempfile::TempDir,
    storage: tempfile::TempDir,
}

impl Fleet {
    pub(crate) fn new() -> Self {
        let fleet = Self {
            home: tempfile::tempdir().unwrap(),
            storage: tempfile::tempdir().unwrap(),
        };
        // The unit file the remote script reads. A LaunchAgent under the
        // tempdir HOME, so `$unit_path` resolves to real state and the
        // operator's own LaunchAgents are never touched.
        let agents = fleet.home.path().join("Library/LaunchAgents");
        std::fs::create_dir_all(&agents).unwrap();
        std::fs::write(
            agents.join(format!("{SERVICE}.plist")),
            format!(
                r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>{SERVICE}</string>
  <key>ProgramArguments</key><array><string>/bin/sh</string></array>
</dict></plist>
"#
            ),
        )
        .unwrap();
        let hostname = String::from_utf8(Command::new("hostname").output().unwrap().stdout)
            .unwrap()
            .trim()
            .to_ascii_lowercase();
        let registry = serde_json::json!({
            "schema_version": 2,
            "targets": [{
                "name": "here",
                "kind": "local",
                "ssh": "nobody@127.0.0.1",
                "release_platform": platform(),
                "hostnames": [hostname],
                "services": [{
                    "label": SERVICE,
                    "name": SERVICE,
                    "kind": "launchd",
                    "path": format!("$HOME/Library/LaunchAgents/{SERVICE}.plist"),
                    "program": "/bin/sh"
                }]
            }],
            "coordinators": []
        });
        std::fs::write(
            fleet.storage.path().join("registry.json"),
            serde_json::to_string_pretty(&registry).unwrap(),
        )
        .unwrap();
        fleet
    }

    /// Declare the endpoint the fleet says this service answers on, the way
    /// `service declare` writes it. This is the only source that distinguishes
    /// a port the unit SERVES from one it merely calls.
    pub(crate) fn declare_endpoint(&self, port: u16) {
        let path = self.storage.path().join("registry.json");
        let mut document: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        document["service_directory"] = serde_json::json!({
            "authority": { "target": "here", "command": "/usr/bin/true" },
            "generation": 1,
            "services": {
                SERVICE: {
                    "active_host": "here",
                    "endpoints": { "here": { "url": format!("http://127.0.0.1:{port}/") } }
                }
            }
        });
        std::fs::write(&path, serde_json::to_string_pretty(&document).unwrap()).unwrap();
    }

    /// Declare the endpoint the way the real fleet does: the directory keyed
    /// by the service's LOGICAL name, and a placement profile carrying the
    /// launchd label that serves it on this host.
    ///
    /// The fixture above deliberately spells the directory key and the unit
    /// label with one string, which is how `brama` -- declared as `brama` and
    /// running as `com.wisent.always-on.brama` -- had no test at all: with one
    /// name there are no two namespaces to disagree.
    pub(crate) fn declare_endpoint_under_logical_name(&self, logical: &str, port: u16) {
        let path = self.storage.path().join("registry.json");
        let mut document: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        document["service_directory"] = serde_json::json!({
            "authority": { "target": "here", "command": "/usr/bin/true" },
            "generation": 1,
            "services": {
                logical: {
                    "active_host": "here",
                    "placement_profile": "test-profile",
                    "endpoints": { "here": { "url": format!("http://127.0.0.1:{port}/") } }
                }
            }
        });
        document["placement_profiles"] = serde_json::json!([{
            "name": "test-profile",
            "services": [logical],
            "hosts": {
                "here": {
                    "units": {
                        logical: {
                            "name": SERVICE,
                            "unit": SERVICE,
                            "kind": "launchd",
                            "path": format!("$HOME/Library/LaunchAgents/{SERVICE}.plist")
                        }
                    }
                }
            }
        }]);
        std::fs::write(&path, serde_json::to_string_pretty(&document).unwrap()).unwrap();
    }

    pub(crate) fn stado(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_stado"))
            .args(args)
            .env_clear()
            .env("HOME", self.home.path())
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("WC_STORAGE_BACKEND", "local")
            .env("WC_LOCAL_STORAGE_PATH", self.storage.path())
            .env(
                "STADO_CONFIG",
                self.storage.path().join("no-such-config.json"),
            )
            .env("WC_PROVIDERS", "local")
            .env("WC_VAST_AUTO_LIST", "false")
            .output()
            .expect("stado binary runs")
    }

    pub(crate) fn serving(&self, extra: &[&str]) -> Output {
        let mut args = vec!["service", "serving", SERVICE, "--host", "here"];
        args.extend_from_slice(extra);
        self.stado(&args)
    }
}

pub(crate) fn platform() -> &'static str {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => "darwin-arm64",
        ("linux", "x86_64") => "linux-amd64",
        (os, arch) => panic!("no release platform mapping for {os}-{arch}"),
    }
}

pub(crate) fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

pub(crate) fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

pub(crate) fn report(out: &Output) -> serde_json::Value {
    let text = stdout(out);
    let parsed: serde_json::Value = serde_json::from_str(&text).unwrap_or_else(|error| {
        panic!(
            "--json output is not JSON ({error}):\nstdout:{text}\nstderr:{}",
            stderr(out)
        )
    });
    parsed
        .as_array()
        .and_then(|rows| rows.first())
        .cloned()
        .unwrap_or_else(|| panic!("--json output has no rows:\n{text}"))
}

/// A real loopback listener, and the port it really holds.
pub(crate) fn live_port() -> (TcpListener, u16) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
    let port = listener.local_addr().unwrap().port();
    (listener, port)
}
