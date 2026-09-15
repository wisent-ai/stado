//! The fleet side the leased-release cases share: the two ways this area
//! runs the built binary, where a lease may be taken, and what the release
//! channel can actually serve.
//!
//! Nothing here asserts a product claim. It is the seam the cases in
//! [`super`] assert through, split out so every file stays inside the three
//! hundred line limit this repository enforces on itself.

use std::path::Path;
use std::process::{Command, Output};
use std::sync::{Mutex, MutexGuard};

use serde_json::Value;

use crate::fixture::{stderr, stdout, BINARY};

/// One lease at a time. Both cases mutate a host, and `scratch create` reaps
/// that host's expired leases before taking a new one, so two unsynchronised
/// cases would sweep each other's account and blame the reaper for working.
static HOST: Mutex<()> = Mutex::new(());

pub fn host_turn() -> MutexGuard<'static, ()> {
    HOST.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// The built binary with the operator's environment intact: the fleet
/// configuration a lease resolves its host through is the operator's, and
/// this area deliberately does not fake it.
pub fn fleet(arguments: &[&str]) -> Output {
    let output = Command::new(env!("CARGO_BIN_EXE_stado"))
        .args(arguments)
        .output()
        .unwrap_or_else(|exc| panic!("stado {arguments:?} did not start: {exc}"));
    eprintln!(
        "stado {arguments:?}: exit {:?}\n{}\n{}",
        output.status.code(),
        stdout(&output),
        stderr(&output)
    );
    output
}

/// The same binary pointed at ONE lease's emitted registry, which is the only
/// document naming a target these commands are allowed to touch. The
/// environment is otherwise the operator's, because the release channel a
/// delivery fetches from is declared there.
pub fn leased(root: &str, arguments: &[&str]) -> Output {
    let output = Command::new(env!("CARGO_BIN_EXE_stado"))
        .args(arguments)
        .env("WC_STORAGE_BACKEND", "local")
        .env("WC_LOCAL_STORAGE_PATH", root)
        .env("STADO_CONFIG", Path::new(root).join("no-such-config.json"))
        .env("STADO_API_URL", release_api())
        .output()
        .unwrap_or_else(|exc| panic!("stado {arguments:?} did not start: {exc}"));
    eprintln!(
        "lease registry {root}; stado {arguments:?}: exit {:?}\n{}\n{}",
        output.status.code(),
        stdout(&output),
        stderr(&output)
    );
    output
}

/// The one JSON document a successful `--json` command printed.
pub fn document(output: &Output, arguments: &[&str]) -> Value {
    assert!(
        output.status.success(),
        "stado {arguments:?} failed: {}{}",
        stdout(output),
        stderr(output)
    );
    serde_json::from_str(&stdout(output))
        .unwrap_or_else(|exc| panic!("stado {arguments:?} printed no JSON document: {exc}"))
}

/// A report a command printed whatever its gate said. `--apply` exits
/// non-zero whenever anything in the pass failed, and the pass's own record
/// of what it delivered is exactly what these cases read.
pub fn report(output: &Output, what: &str) -> Value {
    let said = stdout(output);
    serde_json::from_str(&said)
        .unwrap_or_else(|exc| panic!("{what} printed no report: {exc}\n{said}{}", stderr(output)))
}

/// The release channel origin this run reads and delivers through.
pub fn release_api() -> String {
    let arguments = ["config", "show"];
    let configuration = document(&fleet(&arguments), &arguments);
    configuration["resolved"]["stado_api_url"]
        .as_str()
        .filter(|value| !value.is_empty())
        .expect("the product must resolve its canonical release origin")
        .to_string()
}

/// One host a lease may be taken on, as the fleet itself describes it:
/// target, profile and release platform. A remote host is preferred — a
/// delivery on the machine running these tests would replace the operator's
/// own Stado, which is the whole reason this area leases a target.
pub fn leasable_host() -> (String, String, String) {
    let arguments = ["scratch", "hosts", "--json"];
    let report = document(&fleet(&arguments), &arguments);
    let local = Command::new("hostname")
        .output()
        .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_lowercase())
        .unwrap_or_default();
    let eligible: Vec<&Value> = report["hosts"]
        .as_array()
        .unwrap_or_else(|| panic!("the hosts report carries no hosts array: {report}"))
        .iter()
        .filter(|row| row["eligible"].as_bool().unwrap_or_default())
        .collect();
    let row = eligible
        .iter()
        .find(|row| {
            let target = row["target"].as_str().unwrap_or_default();
            !local.starts_with(target) && !target.starts_with(&local)
        })
        .or_else(|| eligible.first())
        .unwrap_or_else(|| {
            panic!(
                "no registry target is leasable, so this run is blocked rather than \
                 passed: {report}"
            )
        });
    let field = |key: &str| row[key].as_str().unwrap_or_default().to_string();
    (field("target"), field("profile"), field("release_platform"))
}

/// What the channel says about one coordinate: `present`, `absent`, or an
/// `unavailable`/`unreachable` receipt, which must never be read as absence.
pub fn channel_state(uri: &str) -> String {
    let arguments = ["storage", "stat", uri, "--json"];
    let output = fleet(&arguments);
    serde_json::from_str::<Value>(&stdout(&output))
        .ok()
        .and_then(|receipt| receipt["state"].as_str().map(str::to_string))
        .unwrap_or_else(|| {
            panic!(
                "the channel gave no state for {uri}, so its existence is unknown: {}{}",
                stdout(&output),
                stderr(&output)
            )
        })
}

/// Select a completed signed release from the product's recorded runs, then
/// require that the canonical channel serves its manifest and archive.
pub fn deliverable_version(platform: &str) -> String {
    let arguments = ["release", "status", BINARY, "--json"];
    let status = document(&fleet(&arguments), &arguments);
    let mut runs: Vec<&Value> = status["runs"]
        .as_array()
        .expect("release status carries its recorded runs")
        .iter()
        .filter(|run| {
            run["state"] == "completed" && run["platforms"][platform]["state"] == "published"
        })
        .collect();
    runs.sort_by(|left, right| {
        right["created_at"]
            .as_str()
            .cmp(&left["created_at"].as_str())
    });
    for run in runs {
        let version = run["version"].as_str().expect("a recorded release version");
        let base = format!("stado://releases/{BINARY}/{version}/{platform}");
        let manifest = channel_state(&format!("{base}/release.json"));
        let archive = channel_state(&format!("{base}/release.tar.gz"));
        assert!(
            matches!(manifest.as_str(), "present" | "absent")
                && matches!(archive.as_str(), "present" | "absent"),
            "the canonical channel did not establish availability for {base}: {manifest}, {archive}"
        );
        if manifest == "present" && archive == "present" {
            return version.to_string();
        }
    }
    panic!(
        "no completed signed {BINARY} release for {platform} is served by the canonical channel"
    );
}
