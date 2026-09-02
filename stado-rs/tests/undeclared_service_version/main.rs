//! A product Stado delivers, on a host that declares no version for it.
//!
//! Every version diagnostic in this pack is declaration-driven. `host
//! reconcile` and `service converge` enumerate `managed_versions` and compare
//! each entry against the host, and the version reporter reads only the
//! binaries that map names. So a delivered product with no entry produces no
//! row anywhere — and its absence from an otherwise clean report reads as
//! agreement.
//!
//! Measured on `charless-mac-mini` on 2026-09-02, and this is what it cost.
//! `brama` is delivered to that host — `release_control` declares it a release
//! target with install root `/Users/charles/.stado/services/brama` — while the
//! host declares versions for `skarbiec`, `stado` and `weles-worker` only. The
//! running gateway reported 0.2.58 while its own workload registry pinned
//! 0.2.52. `stado host reconcile` named the two declared binaries that had
//! drifted and said nothing whatsoever about the gateway every model call in
//! the fleet passes through, and `stado service converge <host> brama` refused
//! with "declares no brama version" — the right answer to a question nobody
//! thinks to ask. Establishing that by hand took a day of forwarding channels
//! and read-only host probes.
//!
//! What is NOT the gap, and what this pack originally derived from:
//! `com.wisent.always-on.brama`. That label is brama's
//! `legacy_launchd_label`, and the release agent boots it out of the system
//! domain on every pass where it has to bind the stable bind
//! (`release_agent.rs:680-700`, `stop_legacy`); the product is served by the
//! rollout's stable proxy on the declared `stable_bind`, forwarding to
//! whichever candidate port is active, and `cli/service_verify.rs:638-645`
//! records that judging such a product by unit ownership produced a false
//! `misowned` row on 2026-09-01. Deriving delivery from that unit pointed an
//! operator at a unit meant to be dead, and would have gone silent the moment
//! its plist was removed while the version gap stayed. Delivery is therefore
//! derived from `release_control`, and a unit is named for recognition only.
//!
//! These tests defend that it is reported where an operator already looks,
//! that the accounting is exact in both directions, that the two delivery
//! shapes in use are both recognised — because a check that fires on every
//! host is turned off, and a check that fires on none is the defect — that it
//! fires with no unit on the host at all, and that a legacy label is never the
//! witness.
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

/// The product name that gateway is delivered under, and the install root
/// `release_control` states it is staged at. This is the delivery witness the
/// finding derives from: it holds with no launchd unit on the host at all.
const GATEWAY_PRODUCT: &str = "gateway-fake";
const GATEWAY_INSTALL_ROOT: &str = "/Users/charles/.stado/services/gateway-fake";
const GATEWAY_STABLE_BIND: &str = "127.0.0.1:8080";

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

    /// The same host, plus the `release_control` block that states which
    /// products are delivered to it — the witness the finding derives from,
    /// and the one that outlives every launchd unit on the box.
    ///
    /// `versions` replaces the default `managed_versions` so a test can say
    /// exactly which products are accounted for.
    fn declare_registry_with_release_control(
        &self,
        services: &[serde_json::Value],
        versions: serde_json::Value,
        release_control: serde_json::Value,
    ) {
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
                "managed_versions": versions,
                "services": services,
            }],
            "coordinators": [],
            "release_control": release_control,
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

/// `release_control` carrying one blue-green product delivered to this host,
/// exactly as `stado release policy-apply` writes it.
///
/// `legacy_label` is the unit the release agent boots out for the product
/// (`stop_legacy`). Passing one is the brama shape: the label exists on the
/// host, is inactive by declaration, and must never be the reason — or the
/// subject — of a version finding.
fn release_control(product: &str, legacy_label: Option<&str>) -> serde_json::Value {
    let mut policy_target = serde_json::json!({
        "platform": "darwin-arm64",
        "run_as_user": "charles",
        "home": "/Users/charles",
        "state_dir": "/Users/charles/.stado/release-state",
        "runtime_root": "/Users/charles/.stado/run",
        "logs_root": "/Users/charles/.stado/logs",
        "stable_bind": GATEWAY_STABLE_BIND,
        "candidate_ports": [18080, 18081],
        "readiness_path": "/health",
    });
    if let Some(label) = legacy_label {
        let object = policy_target.as_object_mut().expect("target object");
        object.insert("legacy_launchd_label".to_string(), label.into());
        object.insert(
            "legacy_launchd_plist".to_string(),
            format!("/Library/LaunchDaemons/{label}.plist").into(),
        );
    }
    serde_json::json!({
        "schema_version": 1,
        "generation": 4,
        "trusted_keys": {},
        "products": {
            product: {
                "service": product,
                "config_schema": 1,
                "state_schema": 1,
                "install_root": GATEWAY_INSTALL_ROOT,
                "binary": "bin/gateway",
                "launcher": "bin/start-with-vault",
                "binary_env": "GATEWAY_BIN",
                "port_env": "GATEWAY_PORT_OVERRIDE",
                "runtime_env": "GATEWAY_RUNTIME_DIR",
                "strategy": {
                    "kind": "blue-green",
                    "readiness_timeout_seconds": 90,
                    "drain_timeout_seconds": 60,
                    "rollback_window_seconds": 300,
                    "automatic_rollback": true,
                },
                "targets": { HOST: policy_target },
            }
        },
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

/// The case the unit-derived shape missed, and the one the fleet is heading
/// towards: the product is delivered to this host and the host declares no
/// version for it, with NO launchd unit for it anywhere in the document.
///
/// This is what a finished legacy migration looks like — the legacy plist is
/// gone, `stop_legacy` has nothing left to boot out, the rollout's proxy
/// serves the stable bind — and the version gap is entirely unchanged: the
/// bytes still arrive by `stado host release`, which refuses without a
/// declaration ("Delivery carries out a declaration, it does not stand in for
/// one"). A check whose witness was the unit reported this gap today and
/// nothing tomorrow.
#[test]
fn a_delivered_product_with_no_unit_at_all_is_reported() {
    let harness = Harness::new();
    harness.declare_registry_with_release_control(
        &[],
        serde_json::json!({"stado": "0.13.28", "skarbiec": "0.2.8"}),
        release_control(GATEWAY_PRODUCT, None),
    );
    harness.declare_beacon(&[]);

    let findings = harness.findings();
    assert_eq!(
        findings.len(),
        1,
        "a delivered product with no declared version, unit or no unit: {findings:?}"
    );
    assert_eq!(
        findings[0].get("subject").and_then(|v| v.as_str()),
        Some(HOST),
        "the finding must name the host: {:?}",
        findings[0]
    );

    let detail = details(&findings).join("");
    assert!(
        detail.contains("release_control declares"),
        "delivery must be witnessed by the registry's own statement, not by a unit: {detail}"
    );
    assert!(
        detail.contains(GATEWAY_INSTALL_ROOT),
        "names the install root the bytes are staged under: {detail}"
    );
    assert!(
        !detail.contains("out of Stado's own delivery tree"),
        "there is no unit here, so nothing may claim one witnesses the delivery: {detail}"
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
        "says which diagnostics are silent because of it: {detail}"
    );
}

/// A unit that is a product's declared `legacy_launchd_label` is scheduled for
/// bootout, not for delivery, so it is never the witness and never the subject.
///
/// This is brama on `charless-mac-mini` exactly: `release_control` declares
/// the product delivered here AND names `com.wisent.always-on.gateway-fake` as
/// the label `stop_legacy` boots out of the system domain. The row must come
/// from the release-target witness, must not claim the legacy unit runs the
/// delivery tree, must not present it as "the unit that serves it", and must
/// say plainly that the dead unit is not the finding — otherwise the next
/// operator reads an intentionally inactive unit as the defect, which is the
/// day this check cost once already.
#[test]
fn a_legacy_launchd_label_is_not_a_delivered_service_needing_a_version() {
    let harness = Harness::new();
    harness.declare_registry_with_release_control(
        &[unit(GATEWAY, GATEWAY_PROGRAM)],
        serde_json::json!({"stado": "0.13.28", "skarbiec": "0.2.8"}),
        release_control(GATEWAY_PRODUCT, Some(GATEWAY)),
    );
    harness.declare_beacon(&[GATEWAY]);

    let findings = harness.findings();
    assert_eq!(
        findings.len(),
        1,
        "one row for the delivered product, never a second for its legacy unit: {findings:?}"
    );
    let detail = details(&findings).join("");
    assert!(
        detail.contains("release_control declares") && detail.contains(GATEWAY_INSTALL_ROOT),
        "the row must derive from the delivery declaration: {detail}"
    );
    assert!(
        !detail.contains("out of Stado's own delivery tree"),
        "the legacy label must not witness delivery: {detail}"
    );
    assert!(
        !detail.contains("The unit that serves it on this host is"),
        "a unit booted out by the rollout does not serve the product: {detail}"
    );
    assert!(
        detail.contains(GATEWAY_STABLE_BIND) && detail.contains("stable proxy"),
        "names the arrangement that does serve it: {detail}"
    );
    assert!(
        detail.contains(&format!("Its legacy unit {GATEWAY} is booted out")),
        "says the inactive unit is by declaration and is not the finding: {detail}"
    );
}
