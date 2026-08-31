//! `stado host forward-close` against real marker files and a real listener.
//!
//! Every test drives the built `stado` binary. The registry target's
//! `hostnames` name THIS machine, so the remote half runs locally through the
//! same `/bin/bash -s` the ssh branch asks the login shell for, and the marker
//! the command deletes is a real file this test made under a tempdir HOME.
//!
//! There is no fake process table and no stub marker. Where a listener is
//! needed it is a `TcpListener` this test binds, so "the port is still
//! listening" is the kernel's answer and not an assertion about nothing.
//!
//! What is defended: closing a recorded forward removes BOTH markers so
//! `host inventory` stops reporting an endpoint that no longer exists; a name
//! nothing recorded is refused rather than reported as closed; a port still
//! held after the markers are gone fails loudly instead of quietly; and the
//! ssh match is the whole `-R` specification, never the program name, because
//! this machine runs other forwards.

use std::net::TcpListener;
use std::path::PathBuf;
use std::process::{Command, Output};

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
                "slots": 1,
                "services": []
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

    fn forwards(&self) -> PathBuf {
        let directory = self.home.path().join(".stado/forwards");
        std::fs::create_dir_all(&directory).unwrap();
        directory
    }

    /// Both markers exactly as `forward-local` leaves them. HOME is the same
    /// tempdir for the local and the "remote" side here, which is what makes
    /// this a real round trip rather than a simulated one.
    fn record(&self, name: &str, remote_port: u16, local_port: u16) -> (PathBuf, PathBuf) {
        let directory = self.forwards();
        let remote = directory.join(format!("{name}.url"));
        let local = directory.join(format!("{name}.local"));
        std::fs::write(&remote, format!("http://127.0.0.1:{remote_port}\n")).unwrap();
        std::fs::write(&local, format!("http://127.0.0.1:{local_port}\n")).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            for path in [&remote, &local] {
                std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
            }
        }
        (remote, local)
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

    fn close(&self, name: &str, extra: &[&str]) -> Output {
        let mut args = vec!["host", "forward-close", "here", name];
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

fn said(out: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

fn report(out: &Output) -> serde_json::Value {
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    let start = text
        .find('{')
        .unwrap_or_else(|| panic!("no JSON in:\n{text}"));
    let end = text.rfind('}').unwrap();
    serde_json::from_str(&text[start..=end])
        .unwrap_or_else(|error| panic!("bad JSON ({error}):\n{text}"))
}

/// A free port that is provably free: bound, read back, released.
fn free_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    port
}

#[test]
fn closing_a_recorded_forward_removes_both_markers_so_inventory_stops_reporting_it() {
    let fleet = Fleet::new();
    let port = free_port();
    let (remote, local) = fleet.record("oko-oauth", port, port);
    assert!(remote.exists() && local.exists());

    let out = fleet.close("oko-oauth", &["--json"]);
    assert!(out.status.success(), "{}", said(&out));
    let row = report(&out);
    assert_eq!(row["name"], "oko-oauth");
    assert_eq!(row["remote_port"], serde_json::json!(port));
    assert_eq!(row["local_port"], serde_json::json!(port));
    assert_eq!(
        row["remote_marker_removed"],
        serde_json::json!(true),
        "{row}"
    );
    assert_eq!(
        row["local_marker_removed"],
        serde_json::json!(true),
        "{row}"
    );
    // The whole point: nothing is left asserting an endpoint.
    assert!(!remote.exists(), "the host marker survived the close");
    assert!(!local.exists(), "the local marker survived the close");
    // And the port is confirmed reclaimed by re-reading the host.
    assert_eq!(
        row["remote_port_still_listening"],
        serde_json::json!(false),
        "{row}"
    );
}

#[test]
fn a_name_nothing_recorded_is_refused_rather_than_reported_closed() {
    let fleet = Fleet::new();
    fleet.forwards();
    let out = fleet.close("never-opened", &[]);
    assert!(!out.status.success());
    let text = said(&out);
    assert!(text.contains("no forward named"), "{text}");
    assert!(text.contains("never-opened"), "{text}");
    // It points at the command that lists what does exist.
    assert!(text.contains("host inventory"), "{text}");
}

#[test]
fn closing_twice_refuses_the_second_time_instead_of_claiming_success() {
    let fleet = Fleet::new();
    let port = free_port();
    fleet.record("twice", port, port);
    assert!(fleet.close("twice", &[]).status.success());
    let again = fleet.close("twice", &[]);
    assert!(
        !again.status.success(),
        "a second close must not claim to have closed anything"
    );
    assert!(said(&again).contains("no forward named"));
}

#[test]
fn a_port_still_held_after_the_markers_go_fails_loudly() {
    let fleet = Fleet::new();
    // A real listener this test holds for the length of the assertion, so the
    // command's verdict is the kernel's answer. No ssh of ours owns it, which
    // is exactly the state that must not be reported as a clean close.
    let held = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = held.local_addr().unwrap().port();
    let (remote, local) = fleet.record("occupied", port, port);

    let out = fleet.close("occupied", &["--json"]);
    let row = report(&out);
    assert_eq!(
        row["remote_port_still_listening"],
        serde_json::json!(true),
        "{row}"
    );
    assert!(
        !out.status.success(),
        "a port still listening must not exit zero"
    );
    assert!(said(&out).contains("still listening"), "{}", said(&out));
    // The markers are still reconciled: leaving them would keep asserting an
    // endpoint this forward no longer owns.
    assert!(!remote.exists(), "{row}");
    assert!(!local.exists(), "{row}");
    drop(held);
}

#[test]
fn the_close_does_not_signal_forwards_it_was_not_asked_about() {
    let fleet = Fleet::new();
    // Two recorded forwards on different ports. Closing one must leave the
    // other's markers untouched — the ssh match is the whole -R spec, and a
    // match on the program name would take every channel on this machine down.
    let first = free_port();
    let second = free_port();
    let (remote_a, local_a) = fleet.record("keep-me", second, second);
    fleet.record("close-me", first, first);

    assert!(fleet.close("close-me", &[]).status.success());
    assert!(
        remote_a.exists(),
        "another forward's host marker was removed"
    );
    assert!(
        local_a.exists(),
        "another forward's local marker was removed"
    );
}
