//! Observable `stado host weles-browser-runtime` readiness reports.
//!
//! The fixture is a real local registry target with the installed Weles release
//! declaration and Playwright cache markers under an isolated HOME. The built
//! Stado binary reads both through its production host channel.

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
                "hostnames": [hostname],
                "release_platform": platform(),
                "services": []
            }],
            "coordinators": []
        });
        std::fs::write(
            fleet.storage.path().join("registry.json"),
            serde_json::to_string_pretty(&registry).unwrap(),
        )
        .unwrap();

        let declaration = serde_json::json!({
            "browsers": [
                {"name": "chromium", "revision": "1217", "installByDefault": true},
                {"name": "chromium-headless-shell", "revision": "1217", "installByDefault": true},
                {"name": "firefox", "revision": "1511", "installByDefault": true},
                {"name": "webkit", "revision": "2272", "installByDefault": true},
                {"name": "ffmpeg", "revision": "1011", "installByDefault": true}
            ]
        });
        let declaration_path = fleet
            .home
            .path()
            .join("weles/node_modules/playwright-core/browsers.json");
        std::fs::create_dir_all(declaration_path.parent().unwrap()).unwrap();
        std::fs::write(
            declaration_path,
            serde_json::to_vec_pretty(&declaration).unwrap(),
        )
        .unwrap();
        fleet.mark_present("ffmpeg-1011");
        fleet
    }

    fn mark_present(&self, directory: &str) {
        let marker = self
            .home
            .path()
            .join("Library/Caches/ms-playwright")
            .join(directory)
            .join("INSTALLATION_COMPLETE");
        std::fs::create_dir_all(marker.parent().unwrap()).unwrap();
        std::fs::write(marker, b"").unwrap();
    }

    fn runtime(&self, extra: &[&str]) -> Output {
        let mut args = vec!["host", "weles-browser-runtime", "here", "--json"];
        args.extend_from_slice(extra);
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
}

fn platform() -> &'static str {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => "darwin-arm64",
        ("linux", "x86_64") => "linux-amd64",
        (os, arch) => panic!("no release platform mapping for {os}-{arch}"),
    }
}

fn report(output: &Output) -> serde_json::Value {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "runtime report is not JSON: {error}\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    })
}

#[test]
fn recording_ready_without_any_browser_engine_is_named_and_refused() {
    let fleet = Fleet::new();
    let output = fleet.runtime(&[]);
    assert!(!output.status.success());
    let report = report(&output);
    assert_eq!(report["runtime"], "browser_engine_missing");
    assert_eq!(report["required"], serde_json::json!(["ffmpeg"]));
    assert_eq!(report["required_state"], "complete");
    assert_eq!(report["browser_engine_state"], "missing");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains(
        "here: required Playwright components are complete, but no Chromium, Firefox, or WebKit engine is installed, so `browserContext.newPage` cannot open a page; install Chromium with `stado host weles-browser-runtime here --component chromium --repair`."
    ), "{stderr}");
}

#[test]
fn a_named_missing_engine_is_incomplete_and_names_the_effective_repair() {
    let fleet = Fleet::new();
    let output = fleet.runtime(&["--component", "chromium"]);
    assert!(!output.status.success());
    let report = report(&output);
    assert_eq!(report["runtime"], "incomplete");
    assert_eq!(report["required"], serde_json::json!(["chromium"]));
    assert_eq!(report["required_state"], "incomplete");
    assert_eq!(report["browser_engine_state"], "missing");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains(
            "repair it with `stado host weles-browser-runtime here --component chromium --repair`."
        ),
        "{stderr}"
    );
}

#[test]
fn a_present_engine_and_complete_required_list_are_ready() {
    let fleet = Fleet::new();
    fleet.mark_present("chromium-1217");
    let output = fleet.runtime(&[]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report = report(&output);
    assert_eq!(report["runtime"], "complete");
    assert_eq!(report["required_state"], "complete");
    assert_eq!(report["browser_engine_state"], "present");
}
