//! `registry doctor` keeps the two release mechanisms separate.
//!
//! `release_control.products.<product>.desired` is the version authority for
//! release-agent products. `targets[].managed_versions` is the authority only
//! for products in the compiled `host release` catalog. Requiring both creates
//! two desired versions and recommends commands the catalog refuses.

use std::path::PathBuf;
use std::process::{Command, Output};

const HOST: &str = "mini-fake";
const VERSION_FINDING: &str = "undeclared-service-version";
const LEGACY: &str = "com.wisent.always-on.gateway-fake";
const GATEWAY_PROGRAM: &str =
    "/Users/charles/.stado/services/gateway-fake/current/darwin-arm/bin/start-with-vault";
const MANAGED: &str = "com.wisent.always-on.stado-fake";
const MANAGED_PROGRAM: &str = "/Users/charles/.stado/services/stado/current/darwin-arm64/stado";
const LABEL_STAGED: &str = "com.wisent.always-on.object-api-fake";
const LABEL_STAGED_PROGRAM: &str =
    "/Users/charles/.stado/services/com.wisent.always-on.object-api-fake/current/darwin-arm64/stado";
const ARBITRARY: &str = "com.wisent.always-on.weles-admission-fake";
const ARBITRARY_PROGRAM: &str =
    "/Users/charles/.stado/services/weles-admission/current/darwin-arm/weles-api-launcher";

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

    fn declare_registry(
        &self,
        services: &[serde_json::Value],
        versions: serde_json::Value,
        release_control: Option<serde_json::Value>,
    ) {
        let mut document = serde_json::json!({
            "schema_version": 2,
            "targets": [{
                "name": HOST,
                "kind": "local",
                "ssh": "charles@10.9.9.21",
                "release_platform": "darwin-arm64",
                "hostnames": [format!("{HOST}.local")],
                "role": "always-on",
                "host_heuristic": "always-on",
                "managed_versions": versions,
                "services": services,
            }],
            "coordinators": [],
        });
        if let Some(release_control) = release_control {
            document
                .as_object_mut()
                .expect("registry object")
                .insert("release_control".to_string(), release_control);
        }
        std::fs::write(
            self.root().join("storage/registry.json"),
            serde_json::to_string_pretty(&document).expect("registry document"),
        )
        .expect("seed registry");
    }

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
        let mut command = Command::new(env!("CARGO_BIN_EXE_stado"));
        command
            .args(args)
            .env_clear()
            .env("HOME", root.join("home"))
            .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
            .env("WC_STORAGE_BACKEND", "local")
            .env("WC_LOCAL_STORAGE_PATH", root.join("storage"))
            .env("STADO_CONFIG", root.join("storage/no-such-config.json"));
        command.output().expect("stado binary runs")
    }

    fn findings(&self, kind: &str) -> Vec<serde_json::Value> {
        let output = self.stado(&["registry", "doctor", "--json"]);
        let document: serde_json::Value = serde_json::from_slice(&output.stdout)
            .unwrap_or_else(|error| panic!("doctor emitted no JSON ({error}): {output:?}"));
        document
            .get("findings")
            .and_then(serde_json::Value::as_array)
            .map(|rows| {
                rows.iter()
                    .filter(|row| {
                        row.get("finding").and_then(serde_json::Value::as_str) == Some(kind)
                    })
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
    }
}

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

fn release_control() -> serde_json::Value {
    serde_json::json!({
        "schema_version": 1,
        "generation": 4,
        "trusted_keys": {},
        "products": {
            "gateway-fake": {
                "service": "gateway-fake",
                "config_schema": 1,
                "state_schema": 1,
                "install_root": "/Users/charles/.stado/services/gateway-fake",
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
                    "automatic_rollback": true
                },
                "targets": {
                    HOST: {
                        "platform": "darwin-arm64",
                        "run_as_user": "charles",
                        "home": "/Users/charles",
                        "state_dir": "/Users/charles/.stado/release-state",
                        "runtime_root": "/Users/charles/.stado/run",
                        "logs_root": "/Users/charles/.stado/logs",
                        "stable_bind": "127.0.0.1:8080",
                        "candidate_ports": [18080, 18081],
                        "readiness_path": "/health",
                        "legacy_launchd_label": LEGACY,
                        "legacy_launchd_plist": format!("/Library/LaunchDaemons/{LEGACY}.plist")
                    }
                }
            }
        }
    })
}

#[test]
fn release_control_owns_its_version_and_legacy_unit_liveness() {
    let harness = Harness::new();
    harness.declare_registry(
        &[unit(LEGACY, GATEWAY_PROGRAM)],
        serde_json::json!({}),
        Some(release_control()),
    );
    harness.declare_beacon(&[]);

    assert!(
        harness.findings(VERSION_FINDING).is_empty(),
        "a release-control product must not require a duplicate managed version"
    );
    assert!(
        harness.findings("missing-plist").is_empty(),
        "the release agent intentionally removes the legacy unit"
    );
    assert!(
        harness.findings("unit-not-active").is_empty(),
        "the legacy unit is not a liveness subject"
    );
}

#[test]
fn compiled_managed_product_without_version_is_reported() {
    let harness = Harness::new();
    harness.declare_registry(
        &[unit(MANAGED, MANAGED_PROGRAM)],
        serde_json::json!({}),
        None,
    );
    harness.declare_beacon(&[MANAGED]);

    let findings = harness.findings(VERSION_FINDING);
    assert_eq!(
        findings.len(),
        1,
        "exactly one managed version gap: {findings:?}"
    );
    let detail = findings[0]
        .get("detail")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    assert!(detail.contains(MANAGED), "names the unit: {detail}");
    assert!(
        detail.contains(MANAGED_PROGRAM),
        "names the program: {detail}"
    );
    assert!(
        detail.contains("--binary stado"),
        "recommends a command the catalog accepts: {detail}"
    );
}

#[test]
fn declaring_compiled_managed_product_version_closes_finding() {
    let harness = Harness::new();
    harness.declare_registry(
        &[unit(MANAGED, MANAGED_PROGRAM)],
        serde_json::json!({"stado": "0.15.9"}),
        None,
    );
    harness.declare_beacon(&[MANAGED]);

    assert!(harness.findings(VERSION_FINDING).is_empty());
}

#[test]
fn program_filename_maps_label_staged_unit_to_compiled_product() {
    let harness = Harness::new();
    harness.declare_registry(
        &[unit(LABEL_STAGED, LABEL_STAGED_PROGRAM)],
        serde_json::json!({}),
        None,
    );
    harness.declare_beacon(&[LABEL_STAGED]);

    let findings = harness.findings(VERSION_FINDING);
    assert_eq!(
        findings.len(),
        1,
        "the stado binary still carries the version contract: {findings:?}"
    );
    assert_eq!(
        findings[0]
            .get("product")
            .and_then(serde_json::Value::as_str),
        Some("stado")
    );
}

#[test]
fn arbitrary_service_update_tree_gets_no_invented_semver_contract() {
    let harness = Harness::new();
    harness.declare_registry(
        &[unit(ARBITRARY, ARBITRARY_PROGRAM)],
        serde_json::json!({}),
        None,
    );
    harness.declare_beacon(&[ARBITRARY]);

    assert!(
        harness.findings(VERSION_FINDING).is_empty(),
        "service update tracks its artifact without pretending it is a host-release product"
    );
}
