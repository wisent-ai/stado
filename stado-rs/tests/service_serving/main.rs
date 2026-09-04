//! `stado service serving` against real listening sockets and a real launchd
//! label.
//!
//! Every test drives the built `stado` binary. The registry target's
//! `hostnames` name THIS machine, so `deploy/host_channel.rs` runs the remote
//! script locally through the same `/bin/bash -s` the ssh branch asks the login
//! shell for — the script under test is byte-identical either way, and only the
//! hop disappears. HOME is a tempdir, so the unit file being read is real state
//! this test made.
//!
//! There is no stub socket table and no fake process tree. The "listener" is a
//! `TcpListener` this test binds on loopback, and the pid the command reports
//! as holding it is this test process. The owner walk therefore runs against
//! this machine's real `launchctl list` and this test process's real parent
//! chain — which is precisely the case that must come back `unknown` rather
//! than `serving`, because no launchd job owns a `cargo test` process.
//!
//! What is defended: the defect that started this — a unit whose port is held
//! by a process its label does not own is never reported as serving; a port
//! nothing listens on is `not_serving` and says which port; a holder whose
//! owning label cannot be read is `unknown` and never `serving`; the command
//! exits non-zero on anything but `serving`; it refuses when no port was
//! named instead of inventing an empty pass; and the declared port is found
//! even when the directory and the host spell the service differently, which
//! is the shape every real placement-backed service has.

use std::net::TcpListener;
use std::process::{Command, Output};

/// The label every test addresses, declared on the target itself.
const SERVICE: &str = "com.wisent.always-on.weles";

struct Fleet {
    home: tempfile::TempDir,
    storage: tempfile::TempDir,
}

impl Fleet {
    fn new() -> Self {
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
    fn declare_endpoint(&self, port: u16) {
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
    fn declare_endpoint_under_logical_name(&self, logical: &str, port: u16) {
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

    fn stado(&self, args: &[&str]) -> Output {
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

    fn serving(&self, extra: &[&str]) -> Output {
        let mut args = vec!["service", "serving", SERVICE, "--host", "here"];
        args.extend_from_slice(extra);
        self.stado(&args)
    }
}

fn platform() -> &'static str {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => "darwin-arm64",
        ("linux", "x86_64") => "linux-amd64",
        (os, arch) => panic!("no release platform mapping for {os}-{arch}"),
    }
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn report(out: &Output) -> serde_json::Value {
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
fn live_port() -> (TcpListener, u16) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
    let port = listener.local_addr().unwrap().port();
    (listener, port)
}

#[test]
fn a_port_held_by_a_process_this_unit_does_not_own_is_never_serving() {
    let fleet = Fleet::new();
    // Held by THIS test process. No launchd job owns a `cargo test` process,
    // so the owner walk cannot attribute it to the unit — which is exactly the
    // guarantee that stops one of two units with identical argv from claiming
    // the other's process.
    let (_held, port) = live_port();
    let out = fleet.serving(&["--port", &port.to_string(), "--json"]);
    let row = report(&out);

    assert_ne!(
        row["serving"], "serving",
        "a port this unit's label does not own was reported as serving:\n{row}"
    );
    let judged = &row["ports"][0];
    assert_eq!(judged["port"], serde_json::json!(port));
    assert_ne!(judged["verdict"], "served_by_unit", "{row}");
    // The pid is named, because "not serving" without a pid is not actionable.
    let holder = &judged["holders"][0];
    let pid: u32 = holder["pid"].as_str().unwrap().parse().unwrap();
    assert_eq!(pid, std::process::id(), "the holder pid must be this test");
    assert!(
        !out.status.success(),
        "anything but `serving` must exit non-zero:\n{}",
        stdout(&out)
    );
}

#[test]
fn a_port_nothing_listens_on_is_not_serving_and_names_the_port() {
    let fleet = Fleet::new();
    // Bind, learn the port, then drop the listener: the port is real and
    // provably free rather than picked out of the air.
    let port = {
        let (listener, port) = live_port();
        drop(listener);
        port
    };
    let out = fleet.serving(&["--port", &port.to_string(), "--json"]);
    let row = report(&out);
    assert_eq!(row["serving"], "not_serving", "{row}");
    assert_eq!(row["ports"][0]["verdict"], "dead", "{row}");
    assert!(row["ports"][0]["holders"].as_array().unwrap().is_empty());
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains(&port.to_string()),
        "the failure must name the dead port:\n{}",
        stderr(&out)
    );
}

#[test]
fn a_resolved_foreign_owner_is_named_and_reported_as_not_declared() {
    let fleet = Fleet::new();
    let (_held, port) = live_port();
    let out = fleet.serving(&["--port", &port.to_string(), "--json"]);
    let row = report(&out);
    let holder = &row["ports"][0]["holders"][0];

    // The owner walk climbs this test process's real parent chain. Under a
    // terminal launched by a launchd application job it resolves that job;
    // under a bare shell it resolves nothing. Both answers are correct and the
    // guarantee is the same either way, so this defends the invariant rather
    // than the machine it happens to run on: the port is never credited to the
    // unit, and a resolved owner is named and judged against the registry.
    match holder["owner_state"].as_str().unwrap() {
        "resolved" => {
            let owner = holder["owner"].as_str().unwrap();
            assert!(!owner.is_empty(), "a resolved owner must be named:\n{row}");
            assert_ne!(owner, SERVICE, "{row}");
            // Registry knowledge is this side's, and this label is not in it.
            assert_eq!(holder["owner_declared"], serde_json::json!(false), "{row}");
            assert_eq!(row["ports"][0]["verdict"], "served_by_other", "{row}");
            assert_eq!(row["serving"], "not_serving", "{row}");
            assert!(
                stderr(&out).contains(owner),
                "the failure must name the job that holds the port:\n{}",
                stderr(&out)
            );
        }
        "unknown" => {
            assert_eq!(holder["owner"], "", "{row}");
            assert_eq!(holder["owner_declared"], serde_json::Value::Null, "{row}");
            assert_eq!(row["ports"][0]["verdict"], "owner_unknown", "{row}");
            assert_eq!(row["serving"], "unknown", "{row}");
            assert!(
                stderr(&out).contains("could not be established"),
                "{}",
                stderr(&out)
            );
        }
        other => panic!("unexpected owner_state {other:?}:\n{row}"),
    }
    // Whatever the machine answered, this unit is not serving that port.
    assert_ne!(row["ports"][0]["verdict"], "served_by_unit", "{row}");
    assert_ne!(row["serving"], "serving", "{row}");
    assert!(!out.status.success());
}

#[test]
fn the_port_comes_from_the_declared_endpoint_when_none_is_named() {
    let fleet = Fleet::new();
    let (_held, port) = live_port();
    fleet.declare_endpoint(port);
    // No --port: the fleet's own declaration of what this service answers on
    // is what gets judged.
    let out = fleet.serving(&["--json"]);
    let row = report(&out);
    let ports = row["ports"].as_array().unwrap();
    assert_eq!(ports.len(), 1, "{row}");
    assert_eq!(ports[0]["port"], serde_json::json!(port), "{row}");
    let holder = &ports[0]["holders"][0];
    assert_eq!(
        holder["pid"].as_str().unwrap().parse::<u32>().unwrap(),
        std::process::id(),
        "{row}"
    );
    assert_ne!(row["serving"], "serving", "{row}");
}

/// The declared port must resolve when the directory and the host spell the
/// service differently, because that is how the fleet actually spells it.
///
/// `brama` is declared in the service directory as `brama` and runs on its
/// host as `com.wisent.always-on.brama`. This command took one `name` and used
/// it for both lookups: `declared_matching` against the host's labels and
/// `directory_port` against the directory's keys. Asked by service name it
/// refused with "is not a registry-managed service"; asked by label, with "the
/// service directory declares no endpoint ... name it with --port". So the one
/// service whose declared port was wrong was a service whose declared port
/// this command could not read, and on 2026-08-31 that port pointed at another
/// job for seventeen hours.
#[test]
fn the_declared_endpoint_resolves_when_the_directory_and_the_host_name_it_differently() {
    let fleet = Fleet::new();
    let (_held, port) = live_port();
    fleet.declare_endpoint_under_logical_name("weles", port);
    // Addressed by the launchd label, which is what the host declares, while
    // the endpoint is declared under the logical name.
    let out = fleet.serving(&["--json"]);
    assert!(
        !stderr(&out).contains("declares no endpoint"),
        "the declared endpoint must be found through the placement profile: {}",
        stderr(&out)
    );
    let row = report(&out);
    let ports = row["ports"].as_array().unwrap();
    assert_eq!(ports.len(), 1, "{row}");
    assert_eq!(ports[0]["port"], serde_json::json!(port), "{row}");
}

#[test]
fn naming_no_port_is_refused_rather_than_passed_as_an_empty_check() {
    let fleet = Fleet::new();
    // No --port and no declared endpoint: the command must refuse rather than
    // judge nothing and call it a pass.
    let out = fleet.serving(&["--json"]);
    assert!(!out.status.success());
    let said = format!("{}{}", stdout(&out), stderr(&out));
    assert!(said.contains("declares no endpoint"), "{said}");
    assert!(said.contains("--port"), "{said}");
}

#[test]
fn show_reports_what_the_unit_file_declares_and_does_not_call_it_running() {
    // The defect this capability exists for: `show` reaches no process table,
    // so its word must not be one an operator reads as "serving".
    let fleet = Fleet::new();
    let out = fleet.stado(&["service", "show", SERVICE, "--host", "here", "--json"]);
    let text = stdout(&out);
    let parsed: serde_json::Value = serde_json::from_str(&text)
        .unwrap_or_else(|error| panic!("not JSON ({error}):\n{text}{}", stderr(&out)));
    let row = &parsed[0];
    assert_eq!(row["status"], "declares", "{row}");
    assert_ne!(row["status"], "runs", "{row}");
}
