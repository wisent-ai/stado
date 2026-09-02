//! A service Stado delivers, on a host that declares no version for it.
//!
//! Every version diagnostic in this pack is declaration-driven. `host
//! reconcile` and `service converge` enumerate `managed_versions` and compare
//! each entry against the host, and the version reporter reads only the
//! binaries that map names. So a delivered service with no entry produces no
//! row anywhere — and its absence from an otherwise clean report reads as
//! agreement.
//!
//! Measured on `charless-mac-mini` on 2026-09-02, and this is what it cost.
//! `com.wisent.always-on.brama` runs out of
//! `~/.stado/services/brama/current/<platform>/bin/start-with-skarbiec`; the
//! host declares versions for `skarbiec`, `stado` and `weles-worker` only. The
//! running gateway reported 0.2.58 while its own workload registry pinned
//! 0.2.52. `stado host reconcile` named the two declared binaries that had
//! drifted and said nothing whatsoever about the gateway every model call in
//! the fleet passes through, and `stado service converge <host> brama` refused
//! with "declares no brama version" — the right answer to a question nobody
//! thinks to ask. Establishing that by hand took a day of forwarding channels
//! and read-only host probes.
//!
//! These tests defend that it is now reported where an operator already looks,
//! that the accounting is exact in both directions, and that the two delivery
//! shapes in use are both recognised — because a check that fires on every
//! host is turned off, and a check that fires on none is the defect.
//!
//! No host is contacted: the finding is the registry document against itself,
//! for the same reason `misdeclared-domain` is.

use std::path::PathBuf;
use std::process::{Command, Output};

const HOST: &str = "mini-fake";
const FINDING: &str = "undeclared-service-version";

/// The delivered service with no declared version: the brama shape, where the
/// delivery-tree segment is the product name and the program is a launcher
/// script that is not itself a declared binary.
const GATEWAY: &str = "com.wisent.always-on.gateway-fake";
const GATEWAY_PROGRAM: &str =
    "/Users/charles/.stado/services/gateway-fake/current/darwin-arm/bin/start-with-vault";

/// The other delivery shape: staged under its own label, running a binary the
/// host does declare. Accounted for, and a finding here would fire on every
/// host that has one.
const OBJECT_API: &str = "com.wisent.always-on.object-api-fake";
const OBJECT_API_PROGRAM: &str =
    "/Users/charles/.stado/services/com.wisent.always-on.object-api-fake/current/darwin-arm/stado";

/// A declared binary living outside the delivery tree. Not release-managed
/// under a service name, so it is deliberately not judged.
const VAULT: &str = "com.wisent.always-on.vault-fake";
const VAULT_PROGRAM: &str = "/Users/charles/.stado/bin/skarbiec";

/// A service running something nobody delivered at all — a Homebrew node.
/// Outside the delivery tree, so silent.
const HELPER: &str = "com.wisent.helper-fake";
const HELPER_PROGRAM: &str = "/opt/homebrew/bin/node";

struct Harness {
    dir: tempfile::TempDir,
}

impl Harness {
    fn new() -> Self {
        let harness = Self {
            dir: tempfile::tempdir().expect("temp root"),
        };
        for sub in ["storage", "storage/host_health", "home"] {
            std::fs::create_dir_all(harness.root().join(sub)).expect("temp subdirectory");
        }
        harness
    }

    fn root(&self) -> PathBuf {
        self.dir.path().to_path_buf()
    }

    /// One always-on host declaring `services`, and declaring versions for
    /// `stado` and `skarbiec` only.
    ///
    /// `hostnames` never names this machine, so nothing under test can be
    /// routed at the developer's own launchd.
    fn declare_registry(&self, services: &[serde_json::Value]) {
        let document = serde_json::json!({
            "schema_version": 2,
            "targets": [{
                "name": HOST,
                "kind": "local",
                "ssh": "charles@10.9.9.21",
                "release_platform": "darwin-arm64",
                "hostnames": [format!("{HOST}.local")],
                "slots": 1,
                "role": "always-on",
                "host_heuristic": "always-on",
                "managed_versions": {"stado": "0.13.28", "skarbiec": "0.2.8"},
                "services": services,
            }],
            "coordinators": [],
        });
        std::fs::write(
            self.root().join("storage/registry.json"),
            serde_json::to_string_pretty(&document).expect("registry document"),
        )
        .expect("seed registry");
    }

    /// A fresh beacon reporting every declared unit active, so no liveness
    /// finding can be mistaken for this one.
    fn declare_beacon(&self, active: &[&str]) {
        let units: serde_json::Map<String, serde_json::Value> = active
            .iter()
            .map(|unit| ((*unit).to_string(), serde_json::json!({"state": "active"})))
            .collect();
        let beacon = serde_json::json!({
            "host": HOST,
            "reported_at": chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string(),
            "units": units,
        });
        std::fs::write(
            self.root()
                .join("storage/host_health")
                .join(format!("{HOST}.json")),
            serde_json::to_string(&beacon).expect("beacon document"),
        )
        .expect("seed beacon");
    }

    fn stado(&self, args: &[&str]) -> Output {
        let root = self.root();
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_stado"));
        cmd.args(args)
            .env_clear()
            .env("HOME", root.join("home"))
            .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
            .env("WC_STORAGE_BACKEND", "local")
            .env("WC_LOCAL_STORAGE_PATH", root.join("storage"))
            .env("STADO_CONFIG", root.join("storage/no-such-config.json"));
        cmd.output().expect("stado binary runs")
    }

    /// Every `undeclared-service-version` finding `registry doctor` reports.
    fn findings(&self) -> Vec<serde_json::Value> {
        let out = self.stado(&["registry", "doctor", "--json"]);
        let document: serde_json::Value = serde_json::from_slice(&out.stdout)
            .unwrap_or_else(|error| panic!("doctor emitted no JSON ({error}): {out:?}"));
        document
            .get("findings")
            .and_then(|value| value.as_array())
            .map(|rows| {
                rows.iter()
                    .filter(|row| row.get("finding").and_then(|v| v.as_str()) == Some(FINDING))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
    }
}

/// A `services[]` element as `stado service adopt` writes one.
fn unit(label: &str, program: &str) -> serde_json::Value {
    serde_json::json!({
        "name": label,
        "unit": "",
        "label": label,
        "path": format!("/Library/LaunchDaemons/{label}.plist"),
        "kind": "launchd",
        "program": program,
        "args": [],
        "managed_since": "2026-08-19T00:46:51.797832+00:00",
    })
}

fn details(findings: &[serde_json::Value]) -> Vec<String> {
    findings
        .iter()
        .map(|row| {
            row.get("detail")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// The incident: a delivered gateway with no declared version is named, and it
/// says which host, which unit, which product and what to do.
#[test]
fn a_delivered_service_with_no_declared_version_is_reported() {
    let harness = Harness::new();
    harness.declare_registry(&[unit(GATEWAY, GATEWAY_PROGRAM)]);
    harness.declare_beacon(&[GATEWAY]);

    let findings = harness.findings();
    assert_eq!(
        findings.len(),
        1,
        "exactly the delivered service with no declared version: {findings:?}"
    );
    let row = &findings[0];
    assert_eq!(
        row.get("subject").and_then(|v| v.as_str()),
        Some(HOST),
        "the finding must name the host: {row:?}"
    );

    let detail = details(&findings).join("");
    assert!(detail.contains(GATEWAY), "names the unit: {detail}");
    assert!(
        detail.contains(GATEWAY_PROGRAM),
        "names the program: {detail}"
    );
    assert!(
        detail.contains("declares no version for \"gateway-fake\""),
        "names the product a version would be declared against: {detail}"
    );
    assert!(
        detail.contains("stado host declare-version"),
        "names the command that repairs it: {detail}"
    );
    assert!(
        detail.contains("host reconcile") && detail.contains("service converge"),
        "says which diagnostics are silent because of it, which is the whole cost: {detail}"
    );
}

/// The check must be silent for a host whose delivered services all have a
/// declared version. A finding that fires everywhere is a finding that gets
/// ignored, and then this defect returns with the check still installed.
#[test]
fn a_host_whose_delivered_services_are_declared_is_silent() {
    let harness = Harness::new();
    harness.declare_registry(&[
        unit(OBJECT_API, OBJECT_API_PROGRAM),
        unit(VAULT, VAULT_PROGRAM),
        unit(HELPER, HELPER_PROGRAM),
    ]);
    harness.declare_beacon(&[OBJECT_API, VAULT, HELPER]);

    assert!(
        harness.findings().is_empty(),
        "nothing here runs undeclared bytes out of the delivery tree: {:?}",
        details(&harness.findings())
    );
}

/// Both delivery shapes in use are recognised. The gateway is staged under its
/// product name and runs a launcher; the object API is staged under its own
/// label and runs the declared `stado` binary. Accepting only the first
/// spelling would report the second on every host that has one.
#[test]
fn the_label_staged_shape_running_a_declared_binary_is_accounted_for() {
    let harness = Harness::new();
    harness.declare_registry(&[
        unit(GATEWAY, GATEWAY_PROGRAM),
        unit(OBJECT_API, OBJECT_API_PROGRAM),
    ]);
    harness.declare_beacon(&[GATEWAY, OBJECT_API]);

    let findings = harness.findings();
    let detail = details(&findings).join("");
    assert_eq!(
        findings.len(),
        1,
        "only the gateway is undeclared: {detail}"
    );
    assert!(detail.contains(GATEWAY), "{detail}");
    assert!(
        !detail.contains(OBJECT_API),
        "a service running a declared binary must not be reported: {detail}"
    );
}

/// Declaring the version closes the finding. Without this the check could be
/// satisfied by nothing an operator can actually do.
#[test]
fn declaring_the_version_closes_the_finding() {
    let harness = Harness::new();
    let document = serde_json::json!({
        "schema_version": 2,
        "targets": [{
            "name": HOST,
            "kind": "local",
            "ssh": "charles@10.9.9.21",
            "release_platform": "darwin-arm64",
            "hostnames": [format!("{HOST}.local")],
            "slots": 1,
            "role": "always-on",
            "host_heuristic": "always-on",
            "managed_versions": {"stado": "0.13.28", "gateway-fake": "0.2.58"},
            "services": [unit(GATEWAY, GATEWAY_PROGRAM)],
        }],
        "coordinators": [],
    });
    std::fs::write(
        harness.root().join("storage/registry.json"),
        serde_json::to_string_pretty(&document).expect("registry document"),
    )
    .expect("seed registry");
    harness.declare_beacon(&[GATEWAY]);

    assert!(
        harness.findings().is_empty(),
        "a declared version is exactly what this finding asks for: {:?}",
        details(&harness.findings())
    );
}
