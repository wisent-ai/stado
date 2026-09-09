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
    Command::new(env!("CARGO_BIN_EXE_stado"))
        .args(arguments)
        .output()
        .unwrap_or_else(|exc| panic!("stado {arguments:?} did not start: {exc}"))
}

/// The same binary pointed at ONE lease's emitted registry, which is the only
/// document naming a target these commands are allowed to touch. The
/// environment is otherwise the operator's, because the release channel a
/// delivery fetches from is declared there.
pub fn leased(root: &str, arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_stado"))
        .args(arguments)
        .env("WC_STORAGE_BACKEND", "local")
        .env("WC_LOCAL_STORAGE_PATH", root)
        .env("STADO_CONFIG", Path::new(root).join("no-such-config.json"))
        .output()
        .unwrap_or_else(|exc| panic!("stado {arguments:?} did not start: {exc}"))
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
    std::env::var("STADO_API_URL").expect(
        "STADO_API_URL selects the release channel; this area runs with the fleet environment",
    )
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

/// Whether one coordinate is complete for a delivery to a LEASED target on
/// `platform`: the legacy manifest, its archive and its digest list all
/// served, and no signed pipeline manifest beside them.
///
/// The last condition is the one that is easy to get wrong. Delivery prefers
/// a pipeline manifest wherever it exists and verifies its signature against
/// `registry.release_control.trusted_keys` — which the registry `scratch
/// create` emits does not carry, so such a version refuses with `registry
/// declares no release trust keys` before a byte is fetched.
pub fn legacy_complete(version: &str, platform: &str) -> bool {
    let base = format!("stado://releases/{BINARY}/{version}/{platform}");
    channel_state(&format!("{base}/{BINARY}-v{version}-{platform}.tar.gz")) == "present"
        && channel_state(&format!("{base}/release-manifest-{platform}.json")) == "present"
        && channel_state(&format!("{base}/SHA256SUMS")) == "present"
        && channel_state(&format!("{base}/release.json")) == "absent"
}

/// The two newest versions the channel can deliver to a leased target on
/// `platform`, newest first: the one to declare, and the older one to
/// bootstrap the account with.
///
/// Walked down from this build's own version because the channel publishes no
/// listing endpoint. A channel that cannot serve two such coordinates blocks
/// the run rather than letting a case pretend around it.
pub fn deliverable_versions(platform: &str) -> (String, String) {
    let (major, minor, patch) = {
        let mut parts = env!("CARGO_PKG_VERSION").split('.');
        let mut next = || parts.next().unwrap_or("0").parse::<u32>().unwrap_or(0);
        (next(), next(), next())
    };
    let mut found: Vec<String> = Vec::new();
    for candidate in (0..=patch).rev() {
        let version = format!("{major}.{minor}.{candidate}");
        if legacy_complete(&version, platform) {
            found.push(version);
            if found.len() == 2 {
                return (found.remove(0), found.remove(0));
            }
        }
    }
    panic!(
        "the channel serves fewer than two complete legacy {BINARY} coordinates for \
         {platform} below {major}.{minor}.{patch}, so no delivery can be driven and this \
         run is blocked rather than passed; it served {found:?}"
    );
}
