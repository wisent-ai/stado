//! A product's declared environment that cannot reach the unit serving it.
//!
//! The shape every check in this pack had missed: a declaration nothing ever
//! confronts with the world. `registry doctor` compared the registry's
//! declared units against the beacons' one `state` word per unit
//! (`cli/registry.rs`, `declared_units` reads a record's label and nothing
//! else), and compared products against hosts only after resolving
//! `policy.targets.get(host)` — so the host absent from every target map was
//! the loop's skip condition rather than its finding.
//!
//! Measured on `lukasz-macbook` on 2026-09-02, and the measurement is what
//! fixed this check's shape. `release_control.products.skarbiec.environment`
//! declares `SKARBIEC_AUDIT_FILE` and `SKARBIEC_VAULT_FILE`; that product's
//! `targets` map names `charless-mac-mini` only, and no product named
//! `lukasz-macbook` at all. The host nevertheless declared
//! `managed_versions.skarbiec` and ran
//! `com.wisent.compute.service.skarbiec-control-plane`, which the registry
//! adopted as inventory on 2026-09-01 with no program, no args and no
//! environment recorded — three weeks after the plist was hand-created on
//! 10 August. Its `EnvironmentVariables` was an empty dict and its only
//! `ProgramArguments` entry a hand-authored launcher exporting
//! `SKARBIEC_VAULT_FILE` and never `SKARBIEC_AUDIT_FILE`, so the journal went
//! to the unpinned default and reached 573,321,978 bytes while the sibling
//! unit that pins it held 34,486,246. `registry doctor` reported 16
//! divergences and not one of them was this.
//!
//! `release_agent::spawn_release` is the only writer of a product's declared
//! `environment` and the release agent reaches it only through
//! `policy.targets.get(host)` (`release_agent.rs:1878`), which is why the
//! declaration cannot reach such a host by any delivery path.
//!
//! These tests defend that both conditions are reported where an operator
//! already looks, that each names the host, the unit and the variable rather
//! than saying "drift", that a host the policy does name and a unit whose
//! declaration is recorded are both silent — because a check that fires on
//! every host is a check that gets switched off with the defect still in
//! place — and that a unit on another host is reported unread rather than
//! empty, since "nothing was read" and "the unit carries nothing" are
//! different facts and conflating them is this whole class of defect.

use std::path::PathBuf;
use std::process::{Command, Output};

const HOST: &str = "macbook-fake";
const UNRECORDED: &str = "unrecorded-service-environment";
const UNTARGETED: &str = "untargeted-product-host";

/// The product whose policy declares an environment, and the two variables it
/// declares. `AUDIT` is the one the incident lost.
const PRODUCT: &str = "vault-fake";
const AUDIT: &str = "VAULT_FAKE_AUDIT_FILE";
const VAULT: &str = "VAULT_FAKE_VAULT_FILE";

/// The host the product's `targets` map does name, so a row can say where the
/// declaration lands instead of only where it does not.
const NAMED_HOST: &str = "mini-fake";

/// The adopted stub: a record that names a path and declares nothing about
/// what runs there. This is the skarbiec control-plane shape exactly.
const ADOPTED: &str = "com.wisent.compute.service.vault-fake-control-plane";

/// A unit whose declaration IS recorded — program and args both — on the same
/// host and for the same product. Nothing is missing from the document about
/// this one, so it must never produce an unrecorded-declaration row.
const RECORDED: &str = "com.wisent.vault-fake-recorded";
const RECORDED_PROGRAM: &str = "/Users/lukasz/.stado/bin/vault-fake";

/// A unit that mentions the product nowhere in its identifier. The product a
/// unit serves is a token of its label, never a substring of it, and a check
/// that fires on a coincidence is a check an operator turns off.
const UNRELATED: &str = "com.wisent.transcript-lake-stream";

struct Harness {
    dir: tempfile::TempDir,
}

impl Harness {
    fn new() -> Self {
        let harness = Self {
            dir: tempfile::tempdir().expect("temp root"),
        };
        for sub in ["storage", "storage/host_health", "home", "units"] {
            std::fs::create_dir_all(harness.root().join(sub)).expect("temp subdirectory");
        }
        harness
    }

    fn root(&self) -> PathBuf {
        self.dir.path().to_path_buf()
    }

    /// Write a launchd plist for a unit under test, and return its path.
    ///
    /// `env` is what the plist's `EnvironmentVariables` carries; an empty
    /// slice writes the empty dict the incident's plist held.
    fn plist(&self, label: &str, program: &str, env: &[(&str, &str)]) -> String {
        let entries = env
            .iter()
            .map(|(name, value)| format!("    <key>{name}</key><string>{value}</string>"))
            .collect::<Vec<String>>()
            .join("\n");
        let body = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>{label}</string>
  <key>EnvironmentVariables</key>
  <dict>
{entries}
  </dict>
  <key>ProgramArguments</key>
  <array><string>{program}</string></array>
</dict>
</plist>
"#
        );
        let path = self.root().join("units").join(format!("{label}.plist"));
        std::fs::write(&path, body).expect("seed plist");
        path.display().to_string()
    }

    /// One always-on host declaring `services`, its `managed_versions` and a
    /// `release_control` block.
    ///
    /// `hostnames` names this machine's real hostname when `is_self` is set,
    /// which is the only way the command under test will open a unit file:
    /// the local read is gated on the registry resolving this process's host
    /// to the target being checked.
    fn declare(
        &self,
        services: &[serde_json::Value],
        versions: serde_json::Value,
        release_control: serde_json::Value,
        is_self: bool,
    ) {
        let hostnames = if is_self {
            vec![
                hostname(),
                hostname().trim_end_matches(".local").to_string(),
            ]
        } else {
            vec![format!("{HOST}.local")]
        };
        let document = serde_json::json!({
            "schema_version": 2,
            "targets": [{
                "name": HOST,
                "kind": "local",
                "ssh": "lukasz@10.9.9.30",
                "release_platform": "darwin-arm64",
                "hostnames": hostnames,
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
    /// finding can be mistaken for one of these.
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

    /// Every finding of KIND that `registry doctor` reports.
    fn findings(&self, kind: &str) -> Vec<serde_json::Value> {
        let out = self.stado(&["registry", "doctor", "--json"]);
        let document: serde_json::Value = serde_json::from_slice(&out.stdout)
            .unwrap_or_else(|error| panic!("doctor emitted no JSON ({error}): {out:?}"));
        document
            .get("findings")
            .and_then(|value| value.as_array())
            .map(|rows| {
                rows.iter()
                    .filter(|row| row.get("finding").and_then(|v| v.as_str()) == Some(kind))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
    }
}

/// This machine's hostname, by the same call `registry doctor` resolves its
/// own target with.
fn hostname() -> String {
    let out = Command::new("hostname")
        .output()
        .expect("hostname runs on this platform");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// A `services[]` element as `stado service adopt` writes one.
///
/// `program` empty is the adopted-stub shape: the record names a path and
/// declares nothing about what runs there.
fn unit(label: &str, program: &str, path: &str) -> serde_json::Value {
    serde_json::json!({
        "name": label,
        "unit": "",
        "label": label,
        "path": path,
        "kind": "launchd",
        "program": program,
        "args": [],
        "managed_since": "2026-09-01T23:00:10.518396+00:00",
    })
}

/// `release_control` carrying one product that declares an environment.
///
/// `targets` is the list of hosts the product's `targets` map names — the
/// lookup that gates both delivery and, until this check, every diagnostic.
fn release_control(targets: &[&str]) -> serde_json::Value {
    let policy_target = serde_json::json!({
        "platform": "darwin-arm64",
        "run_as_user": "lukasz",
        "home": "/Users/lukasz",
        "state_dir": "/Users/lukasz/.stado/release-state",
        "runtime_root": "/Users/lukasz/.stado/run",
        "logs_root": "/Users/lukasz/.stado/logs",
        "stable_bind": "127.0.0.1:8799",
        "candidate_ports": [18799, 18800],
        "readiness_path": "/health",
    });
    let targets: serde_json::Map<String, serde_json::Value> = targets
        .iter()
        .map(|host| ((*host).to_string(), policy_target.clone()))
        .collect();
    serde_json::json!({
        "schema_version": 1,
        "generation": 4,
        "trusted_keys": {},
        "products": {
            PRODUCT: {
                "service": PRODUCT,
                "config_schema": 1,
                "state_schema": 1,
                "install_root": "/Users/lukasz/.stado/services/vault-fake",
                "binary": "bin/vault-fake",
                "launcher": "bin/start",
                "binary_env": "VAULT_FAKE_BIN",
                "port_env": "VAULT_FAKE_PORT_OVERRIDE",
                "runtime_env": "VAULT_FAKE_RUNTIME_DIR",
                "environment": {
                    AUDIT: "{home}/.stado/vault-fake.audit.jsonl",
                    VAULT: "{home}/.stado/vault-fake.vault.json",
                },
                "strategy": {
                    "kind": "blue-green",
                    "readiness_timeout_seconds": 90,
                    "drain_timeout_seconds": 60,
                    "rollback_window_seconds": 300,
                    "automatic_rollback": true,
                },
                "targets": targets,
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

/// The incident's second half: a host running a product's unit while no
/// product target names it, so the declared environment cannot reach it.
///
/// The row must name the host, the unit and both variables, and it must say
/// where the declaration does land — a verdict an operator cannot act on is
/// the same defect again.
#[test]
fn a_host_no_product_target_names_is_reported() {
    let harness = Harness::new();
    let path = harness.plist(ADOPTED, "/Users/lukasz/.stado/bin/launcher", &[]);
    harness.declare(
        &[unit(ADOPTED, "", &path)],
        serde_json::json!({PRODUCT: "0.1.3", "stado": "0.13.46"}),
        release_control(&[NAMED_HOST]),
        false,
    );
    harness.declare_beacon(&[ADOPTED]);

    let findings = harness.findings(UNTARGETED);
    assert_eq!(findings.len(), 1, "expected one row, got {findings:?}");
    let detail = &details(&findings)[0];
    for expected in [HOST, ADOPTED, PRODUCT, AUDIT, VAULT, NAMED_HOST] {
        assert!(
            detail.contains(expected),
            "row must name {expected}: {detail}"
        );
    }
    assert!(
        detail.contains("no release_control product names"),
        "row must state the target gap plainly: {detail}"
    );
    assert_eq!(
        findings[0].get("subject").and_then(|v| v.as_str()),
        Some(HOST),
        "the finding is about the host"
    );
}

/// The same host, once a product target names it. The declaration now has a
/// delivery path, so the target row must vanish: this is the assertion that
/// keeps the check from firing on every host in the fleet.
#[test]
fn a_host_a_product_target_names_is_silent() {
    let harness = Harness::new();
    let path = harness.plist(ADOPTED, "/Users/lukasz/.stado/bin/launcher", &[]);
    harness.declare(
        &[unit(ADOPTED, "", &path)],
        serde_json::json!({PRODUCT: "0.1.3"}),
        release_control(&[HOST, NAMED_HOST]),
        false,
    );
    harness.declare_beacon(&[ADOPTED]);

    assert!(
        harness.findings(UNTARGETED).is_empty(),
        "a named host has a delivery path: {:?}",
        details(&harness.findings(UNTARGETED))
    );
}

/// The incident's first half, with the unit file on this machine: an adopted
/// stub whose recorded declaration is empty, against a product policy that
/// declares an environment.
///
/// The row must name the variable the unit does not carry. Reporting "drift"
/// here, or reporting the declared set without saying which of it is absent,
/// leaves the operator exactly where the 573 MB journal left them.
#[test]
fn an_adopted_unit_missing_a_declared_variable_names_it() {
    let harness = Harness::new();
    // The incident's plist exactly: an empty EnvironmentVariables dict and a
    // hand-authored launcher that pins the vault and never the audit file.
    let path = harness.plist(ADOPTED, "/Users/lukasz/.stado/bin/launcher", &[]);
    harness.declare(
        &[unit(ADOPTED, "", &path)],
        serde_json::json!({PRODUCT: "0.1.3"}),
        // Named, so only the unrecorded-declaration half is under test here.
        release_control(&[HOST]),
        true,
    );
    harness.declare_beacon(&[ADOPTED]);

    let findings = harness.findings(UNRECORDED);
    assert_eq!(findings.len(), 1, "expected one row, got {findings:?}");
    let detail = &details(&findings)[0];
    for expected in [HOST, ADOPTED, PRODUCT, AUDIT, &path] {
        assert!(
            detail.contains(expected),
            "row must name {expected}: {detail}"
        );
    }
    assert!(
        detail.contains("no environment variables at all"),
        "row must say what the unit actually carries: {detail}"
    );
    assert!(
        detail.contains("recording no program"),
        "row must say the record itself is empty: {detail}"
    );
    assert_eq!(
        findings[0]
            .get("detail")
            .and_then(|v| v.as_str())
            .map(|d| d.contains("was not read")),
        Some(false),
        "the unit was readable here, so it must not be reported unread"
    );
}

/// A unit that carries only one of the two declared variables. The row must
/// name the absent one and not the present one, because the operator's next
/// action is to pin exactly that variable.
#[test]
fn only_the_variables_the_unit_lacks_are_named_as_missing() {
    let harness = Harness::new();
    let path = harness.plist(
        ADOPTED,
        "/Users/lukasz/.stado/bin/launcher",
        &[(VAULT, "/Users/lukasz/.stado/vault-fake.vault.json")],
    );
    harness.declare(
        &[unit(ADOPTED, "", &path)],
        serde_json::json!({PRODUCT: "0.1.3"}),
        release_control(&[HOST]),
        true,
    );
    harness.declare_beacon(&[ADOPTED]);

    let detail = details(&harness.findings(UNRECORDED))
        .pop()
        .expect("one row");
    assert!(
        detail.contains(&format!(
            "{AUDIT} is declared and the unit does not carry it"
        )),
        "the absent variable must be named as absent: {detail}"
    );
    assert!(
        detail.contains(&format!("carries {VAULT}")),
        "the variable the unit does hold must be reported as held: {detail}"
    );
}

/// A unit on another host is reported unread, never empty.
///
/// `registry doctor` answers for the fleet out of the store and never sshes,
/// and no object in the store carries a unit's environment: the beacon
/// publishes one `state` word per unit and the registry's service record has
/// no environment field. Claiming "carries nothing" from a read that did not
/// happen is the defect this whole check exists to catch, so the row must say
/// the read did not happen and point at the command that can.
#[test]
fn a_unit_on_another_host_is_reported_unread_not_empty() {
    let harness = Harness::new();
    harness.declare(
        &[unit(
            ADOPTED,
            "",
            "/Users/lukasz/Library/LaunchAgents/absent.plist",
        )],
        serde_json::json!({PRODUCT: "0.1.3"}),
        release_control(&[HOST]),
        false,
    );
    harness.declare_beacon(&[ADOPTED]);

    let detail = details(&harness.findings(UNRECORDED))
        .pop()
        .expect("one row");
    assert!(
        detail.contains("was not read"),
        "an unread unit must be reported unread: {detail}"
    );
    assert!(
        detail.contains(&format!("stado service env {HOST} {ADOPTED}")),
        "the row must name the command that can read it: {detail}"
    );
    assert!(
        !detail.contains("no environment variables at all"),
        "an unread unit must never be reported as carrying nothing: {detail}"
    );
}

/// A unit whose declaration IS recorded produces no unrecorded row.
///
/// `service deploy` records the program and args it rendered the unit from, so
/// there is something in the document to diff. Firing here would report every
/// properly declared service in the fleet.
#[test]
fn a_unit_whose_declaration_is_recorded_is_silent() {
    let harness = Harness::new();
    let path = harness.plist(RECORDED, RECORDED_PROGRAM, &[]);
    harness.declare(
        &[unit(RECORDED, RECORDED_PROGRAM, &path)],
        serde_json::json!({PRODUCT: "0.1.3"}),
        release_control(&[HOST]),
        true,
    );
    harness.declare_beacon(&[RECORDED]);

    assert!(
        harness.findings(UNRECORDED).is_empty(),
        "a recorded declaration has something to diff: {:?}",
        details(&harness.findings(UNRECORDED))
    );
}

/// A host that declares no version for the product is silent, and a unit that
/// does not name the product is silent.
///
/// Both witnesses are required precisely because a product declares an
/// environment on every host in this fleet: keying off the policy alone would
/// fire everywhere, and a check that fires everywhere is switched off with the
/// defect still in place. The unrelated unit also pins that the product is
/// matched as a whole token of the label and never as a substring.
#[test]
fn both_witnesses_are_required() {
    let undeclared = Harness::new();
    let path = undeclared.plist(ADOPTED, "/Users/lukasz/.stado/bin/launcher", &[]);
    undeclared.declare(
        &[unit(ADOPTED, "", &path)],
        // No version declared for the product: the host never said it runs it.
        serde_json::json!({"stado": "0.13.46"}),
        release_control(&[NAMED_HOST]),
        true,
    );
    undeclared.declare_beacon(&[ADOPTED]);
    assert!(
        undeclared.findings(UNRECORDED).is_empty() && undeclared.findings(UNTARGETED).is_empty(),
        "a host declaring no version for the product said nothing to check: {:?}",
        details(&undeclared.findings(UNTARGETED))
    );

    let unrelated = Harness::new();
    let path = unrelated.plist(UNRELATED, "/Users/lukasz/.stado/bin/stream", &[]);
    unrelated.declare(
        &[unit(UNRELATED, "", &path)],
        serde_json::json!({PRODUCT: "0.1.3"}),
        release_control(&[NAMED_HOST]),
        true,
    );
    unrelated.declare_beacon(&[UNRELATED]);
    assert!(
        unrelated.findings(UNRECORDED).is_empty() && unrelated.findings(UNTARGETED).is_empty(),
        "no unit here names the product: {:?}",
        details(&unrelated.findings(UNTARGETED))
    );
}

/// A product that declares no environment produces neither row.
///
/// There is nothing that could fail to reach the host, so both conditions are
/// vacuous and reporting them would be noise.
#[test]
fn a_product_declaring_no_environment_is_silent() {
    let harness = Harness::new();
    let path = harness.plist(ADOPTED, "/Users/lukasz/.stado/bin/launcher", &[]);
    let mut control = release_control(&[NAMED_HOST]);
    control["products"][PRODUCT]["environment"] = serde_json::json!({});
    harness.declare(
        &[unit(ADOPTED, "", &path)],
        serde_json::json!({PRODUCT: "0.1.3"}),
        control,
        true,
    );
    harness.declare_beacon(&[ADOPTED]);

    assert!(
        harness.findings(UNRECORDED).is_empty() && harness.findings(UNTARGETED).is_empty(),
        "a product with no declared environment has nothing to fail to reach"
    );
}
