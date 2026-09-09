//! The fleet `stado service serving` is asked about, and the real socket it
//! is asked about.
//!
//! One machine is involved and it is this one: the registry target's
//! `hostnames` carry this host's own kernel name, so `deploy/host_channel.rs`
//! runs the remote script locally through the same `/bin/bash -s` the ssh
//! branch asks the login shell for — the script under test is byte-identical
//! either way and only the hop disappears. `HOME` is a tempdir, so the unit
//! file being read is real state this test made and the operator's own
//! LaunchAgents are never touched.
//!
//! There is no stub socket table and no fake process tree. The "listener" is
//! a `TcpListener` this test binds on loopback, and the pid the command
//! reports as holding it is this test process.

use std::net::TcpListener;
use std::process::{Command, Output};

use serde_json::{json, Value};

/// The label every case addresses, declared on the target itself.
pub const SERVICE: &str = "com.wisent.always-on.weles";

pub struct Fleet {
    home: tempfile::TempDir,
    storage: tempfile::TempDir,
}

impl Fleet {
    pub fn new() -> Self {
        let fleet = Self {
            home: tempfile::tempdir().unwrap(),
            storage: tempfile::tempdir().unwrap(),
        };
        // The unit file the remote script reads. A LaunchAgent under the
        // tempdir HOME, so `$unit_path` resolves to real state.
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
        let registry = json!({
            "schema_version": 2,
            "targets": [{
                "name": "here",
                "kind": "local",
                // Declared because the schema requires the service
                // directory's authority target to carry an ssh path
                // (`service_resolution.rs`: "authority.target: must declare
                // an SSH connection path"). It is never dialed: the
                // `hostnames` below name this machine, so the channel takes
                // its local branch.
                "ssh": "nobody@127.0.0.1",
                "release_platform": platform(),
                "hostnames": [kernel_hostname()],
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
        fleet.write_registry(&registry);
        fleet
    }

    fn write_registry(&self, document: &Value) {
        std::fs::write(
            self.storage.path().join("registry.json"),
            serde_json::to_string_pretty(document).unwrap(),
        )
        .unwrap();
    }

    fn registry(&self) -> Value {
        serde_json::from_str(
            &std::fs::read_to_string(self.storage.path().join("registry.json")).unwrap(),
        )
        .unwrap()
    }

    /// Declare the endpoint the fleet says this service answers on, the way
    /// `service declare` writes it. This is the only source that
    /// distinguishes a port the unit SERVES from one it merely calls.
    pub fn declare_endpoint(&self, port: u16) {
        let mut document = self.registry();
        document["service_directory"] = json!({
            "authority": { "target": "here", "command": authority_command() },
            "generation": 1,
            "services": {
                SERVICE: {
                    "active_host": "here",
                    "endpoints": { "here": { "url": format!("http://127.0.0.1:{port}/") } }
                }
            }
        });
        self.write_registry(&document);
    }

    /// Declare the endpoint the way the real fleet does: the directory keyed
    /// by the service's LOGICAL name, and a placement profile carrying the
    /// launchd label that serves it on this host.
    ///
    /// [`Fleet::declare_endpoint`] deliberately spells the directory key and
    /// the unit label with one string, which is how `brama` -- declared as
    /// `brama` and running as `com.wisent.always-on.brama` -- had no test at
    /// all: with one name there are no two namespaces to disagree.
    pub fn declare_endpoint_under_logical_name(&self, logical: &str, port: u16) {
        let mut document = self.registry();
        document["service_directory"] = json!({
            "authority": { "target": "here", "command": authority_command() },
            "generation": 1,
            "services": {
                logical: {
                    "active_host": "here",
                    "placement_profile": "test-profile",
                    "endpoints": { "here": { "url": format!("http://127.0.0.1:{port}/") } }
                }
            }
        });
        document["placement_profiles"] = json!([{
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
        self.write_registry(&document);
    }

    pub fn stado(&self, args: &[&str]) -> Output {
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

    pub fn serving(&self, extra: &[&str]) -> Output {
        let mut args = vec!["service", "serving", SERVICE, "--host", "here"];
        args.extend_from_slice(extra);
        self.stado(&args)
    }
}

/// This machine's kernel host name, normalized the way the registry validator
/// demands.
fn kernel_hostname() -> String {
    let out = Command::new("/bin/hostname")
        .output()
        .expect("hostname(1) runs");
    String::from_utf8_lossy(&out.stdout).trim().to_ascii_lowercase()
}

/// What the service directory declares its authority runs.
///
/// The built binary under test, because that IS the command a real directory
/// names: the authority is a Stado on another host, reached over the
/// channel. A scripted stand-in here would be a fleet whose authority is not
/// the product.
fn authority_command() -> &'static str {
    env!("CARGO_BIN_EXE_stado")
}

fn platform() -> &'static str {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => "darwin-arm64",
        ("linux", "x86_64") => "linux-amd64",
        (os, arch) => panic!("no release platform mapping for {os}-{arch}"),
    }
}

pub fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

pub fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

/// The first row of the `--json` report, or a panic carrying what was printed.
pub fn report(out: &Output) -> Value {
    let text = stdout(out);
    let parsed: Value = serde_json::from_str(&text).unwrap_or_else(|error| {
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
pub fn live_port() -> (TcpListener, u16) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
    let port = listener.local_addr().unwrap().port();
    (listener, port)
}
