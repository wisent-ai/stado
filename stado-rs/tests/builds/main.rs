//! The fleet's daily build ration, through the built Stado binary and a real
//! isolated queue.
//!
//! This file used to drive build recipes: declare one, let a poller see a new
//! commit, watch a worker build it. Nobody had asked for that machinery and
//! the operator removed it ("to usun ta funkcjonalnosc"), so what is left
//! here is what the ration has to keep doing without it — a compile is
//! counted wherever it is submitted, a spent day refuses the next one, and a
//! queue nobody wants is emptied by one command.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::{json, Value};

fn build_platform() -> &'static str {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => "darwin-arm64",
        ("linux", "x86_64") => "linux-amd64",
        (os, arch) => panic!("build journey has no platform mapping for {os}-{arch}"),
    }
}

struct Journey {
    home: tempfile::TempDir,
    storage: PathBuf,
}

impl Journey {
    fn new() -> Self {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/build-runs");
        fs::create_dir_all(&root).unwrap();
        let home = tempfile::Builder::new()
            .prefix("build-")
            .tempdir_in(root)
            .unwrap();
        let storage = home.path().join("store");
        fs::create_dir_all(&storage).unwrap();
        let hostname =
            String::from_utf8(Command::new("hostname").arg("-f").output().unwrap().stdout)
                .unwrap()
                .trim()
                .to_ascii_lowercase();
        let registry = json!({
            "schema_version": 2,
            "targets": [{
                "name": "build-runner",
                "kind": "local",
                "ssh": "nobody@127.0.0.1",
                "release_platform": build_platform(),
                "hostnames": [hostname],
                "disk_cleanup": {
                    "mode": "off",
                    "check_interval_seconds": 300,
                    "low_free_gb": 10,
                    "target_free_gb": 12,
                    "max_bytes_per_pass": 53687091200_u64,
                    "max_items_per_pass": 50,
                    "max_scan_items": 10000,
                    "cleaners": {}
                }
            }],
            "coordinators": [{
                "name": "build-coordinator",
                "runtime": "cron",
                "interval_seconds": 60,
                "active": true
            }]
        });
        fs::write(
            storage.join("registry.json"),
            serde_json::to_string_pretty(&registry).unwrap(),
        )
        .unwrap();
        Self { home, storage }
    }

    fn command(&self) -> Command {
        let mut command = Command::new(
            std::env::var_os("STADO_TEST_BINARY")
                .unwrap_or_else(|| env!("CARGO_BIN_EXE_stado").into()),
        );
        command
            .env_clear()
            .env("HOME", self.home.path())
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("STADO_CONFIG", self.home.path().join("no-config.json"))
            .env("WC_STORAGE_BACKEND", "local")
            .env("WC_LOCAL_STORAGE_PATH", &self.storage)
            .env("WC_PROVIDERS", "local")
            .env("WC_VAST_AUTO_LIST", "false");
        command
    }

    fn invoke(&self, args: &[&str]) -> Output {
        self.command().args(args).output().unwrap()
    }

    fn invoke_ok(&self, args: &[&str]) -> Output {
        let output = self.invoke(args);
        assert!(
            output.status.success(),
            "stado {} failed\nstdout:\n{}\nstderr:\n{}",
            args.join(" "),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
        output
    }

    fn budget(&self) -> Value {
        serde_json::from_slice(&self.invoke_ok(&["queue", "budget", "--json"]).stdout).unwrap()
    }
}

/// The ceiling has to hold for every submission, not only for the paths that
/// knew they were submitting a build.
///
/// Until 2026-09-21 the count was asked by the callers that knew — a poller,
/// a run-now command, the release pipeline — so a raw `stado submit` carrying
/// a build command, a `stado job rerun` of a build job, or a client older
/// than the ceiling spent the fleet's day and left the counter saying nothing
/// had been spent. That is how six builds went out against a ceiling of
/// three. The charge belongs to the submission itself, so this asks the queue
/// directly.
#[test]
fn a_build_submitted_straight_to_the_queue_is_counted_and_then_refused() {
    let journey = Journey::new();
    let compile = "set -eu; cargo build --release; printf '%s' 0.0.1 > stado-build-version.txt";

    let standing = journey.budget();
    assert_eq!(standing["limit"], 3, "the workshop's standing rule");
    assert_eq!(standing["used"], 0);

    journey.invoke_ok(&["queue", "budget", "--limit", "1"]);
    journey.invoke_ok(&["submit", "--command", compile]);

    let spent = journey.budget();
    assert_eq!(
        spent["used"], 1,
        "a build submitted straight to the queue was not counted: {spent}"
    );
    assert_eq!(spent["remaining"], 0);

    let refused = journey.invoke(&["submit", "--command", compile]);
    assert_ne!(
        refused.status.code(),
        Some(0),
        "a spent day accepted another compile"
    );
    let stderr = String::from_utf8_lossy(&refused.stderr);
    assert!(stderr.contains("daily build budget is spent"), "{stderr}");
    assert_eq!(journey.budget()["used"], 1, "a refused build costs nothing");

    let plain = journey.invoke(&["submit", "--command", "printf hello"]);
    assert_eq!(
        plain.status.code(),
        Some(0),
        "a job that compiles nothing was refused by the build ceiling: {}",
        String::from_utf8_lossy(&plain.stderr)
    );

    journey.invoke_ok(&["queue", "budget", "--limit", "2"]);
    let raised = journey.budget();
    assert_eq!(raised["limit"], 2);
    assert_eq!(raised["used"], 1, "raising the ceiling keeps the count");
}

/// A queue nobody wants is emptied by one command.
///
/// On 2026-09-21 this fleet held 33 queued jobs that were no longer wanted
/// and the only route was `stado cancel <id>` thirty-three times, which is
/// how a queue stays full.
#[test]
fn the_whole_queue_is_cancelled_by_one_command() {
    let journey = Journey::new();
    journey.invoke_ok(&["submit", "--command", "printf one"]);
    journey.invoke_ok(&["submit", "--command", "printf two"]);

    let cancelled = journey.invoke_ok(&["cancel", "--queued"]);
    let said = String::from_utf8_lossy(&cancelled.stdout);
    assert!(said.contains("cancelled 2 queued job(s)"), "{said}");

    let status = String::from_utf8_lossy(&journey.invoke_ok(&["status"]).stdout).to_string();
    assert!(
        !status.contains("queued"),
        "the queue still holds work after cancelling it: {status}"
    );

    let again = journey.invoke_ok(&["cancel", "--queued"]);
    assert!(
        String::from_utf8_lossy(&again.stdout).contains("cancelled 0 queued job(s)"),
        "cancelling an empty queue is not an error"
    );
}
