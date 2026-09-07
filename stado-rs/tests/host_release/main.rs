//! Release host-state integration tests against the built binary and isolated local storage.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Command, Output};

use serde_json::{json, Value};

const HOST: &str = "host-release-test";

struct Fixture {
    _root: tempfile::TempDir,
    storage: PathBuf,
    home: PathBuf,
}

impl Fixture {
    fn new(managed_versions: Value, platform: &str) -> Self {
        let root = tempfile::tempdir().expect("host-release tempdir");
        let storage = root.path().join("storage");
        let home = root.path().join("home");
        fs::create_dir_all(&storage).expect("storage directory");
        fs::create_dir_all(home.join(".stado/bin")).expect("host bin directory");
        let hostname = String::from_utf8(
            Command::new("hostname")
                .output()
                .expect("hostname runs")
                .stdout,
        )
        .expect("hostname is UTF-8")
        .trim()
        .trim_end_matches(".local")
        .to_lowercase()
            + ".local";
        let registry = json!({
            "schema_version": 2,
            "targets": [{
                "name": HOST,
                "kind": "local",
                "ssh": null,
                "hostnames": [hostname],
                "release_platform": platform,
                "managed_versions": managed_versions,
                "services": [],
                // These decoys prove desired state is not read from an ad-hoc field.
                "software": {"stado": "99.99.99"},
                "desired_versions": {"stado": "88.88.88"}
            }],
            "coordinators": []
        });
        fs::write(
            storage.join("registry.json"),
            serde_json::to_vec_pretty(&registry).unwrap(),
        )
        .expect("isolated registry");
        Self {
            _root: root,
            storage,
            home,
        }
    }

    fn stado(&self, args: &[&str]) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_stado"));
        command
            .args(args)
            .env("HOME", &self.home)
            .env("WC_STORAGE_BACKEND", "local")
            .env("WC_LOCAL_STORAGE_PATH", &self.storage)
            .env("STADO_CONFIG", self.storage.join("no-such-config.json"))
            .env_remove("COMPUTE_API_KEY")
            .env_remove("COMPUTE_API_URL")
            .env_remove("WC_PROFILES_DIR");
        command.output().expect("stado binary runs")
    }

    fn registry(&self) -> Value {
        serde_json::from_slice(&fs::read(self.storage.join("registry.json")).unwrap()).unwrap()
    }

    fn install_running_stado(&self) {
        let live = self.home.join(".stado/bin/stado");
        fs::copy(env!("CARGO_BIN_EXE_stado"), &live).expect("install fixture stado");
        fs::set_permissions(&live, fs::Permissions::from_mode(0o700)).unwrap();
        let coordinate = self
            .home
            .join(".stado/releases/stado")
            .join(env!("CARGO_PKG_VERSION"))
            .join(release_platform());
        fs::create_dir_all(&coordinate).unwrap();
        fs::copy(&live, coordinate.join("stado")).expect("stage matching fixture stado");
        fs::write(
            coordinate.join("release-receipt.json"),
            br#"{"installed_at":"2026-09-06T12:00:00Z","delivered_by":"host-release-test"}"#,
        )
        .unwrap();
    }
}

fn release_platform() -> &'static str {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => "darwin-arm64",
        ("linux", "x86_64") => "linux-amd64",
        pair => panic!("host-release fixture has no published platform for {pair:?}"),
    }
}

fn previous_patch(version: &str) -> String {
    let mut parts = version.split('.');
    let major = parts.next().unwrap();
    let minor = parts.next().unwrap();
    let patch: u64 = parts.next().unwrap().parse().unwrap();
    assert!(patch > 0, "fixture package version needs a previous patch");
    format!("{major}.{minor}.{}", patch - 1)
}

fn next_patch(version: &str) -> String {
    let mut parts = version.split('.');
    let major = parts.next().unwrap();
    let minor = parts.next().unwrap();
    let patch: u64 = parts.next().unwrap().parse().unwrap();
    format!("{major}.{minor}.{}", patch + 1)
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn json_stdout(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|error| panic!("stdout is not JSON ({error}): {}", stdout(output)))
}

#[test]
fn release_help_owns_the_moved_operations_and_host_help_does_not() {
    let fixture = Fixture::new(json!({}), release_platform());
    let release = fixture.stado(&["release", "--help"]);
    assert!(release.status.success(), "{}", stderr(&release));
    let release_help = stdout(&release);
    for operation in [
        "declare-version",
        "promote-version",
        "activate-staged",
        "verify-platform",
        "host-state",
        "provenance",
    ] {
        assert!(
            release_help.contains(operation),
            "release help omitted {operation}"
        );
    }

    let host = fixture.stado(&["host", "--help"]);
    assert!(host.status.success(), "{}", stderr(&host));
    let host_help = stdout(&host);
    for retired in [
        "declare-version",
        "promote-version",
        "activate-staged-release",
        "verify-release-platform",
        "release",
        "software",
        "provenance",
    ] {
        assert!(
            !host_help
                .lines()
                .any(|line| line.split_whitespace().next() == Some(retired)),
            "host help still exposes {retired}: {host_help}"
        );
    }
}

#[test]
fn declare_version_persists_only_the_managed_versions_declaration() {
    let fixture = Fixture::new(json!({}), release_platform());
    let output = fixture.stado(&[
        "release",
        "declare-version",
        "--host",
        HOST,
        "--binary",
        "stado",
        "--version",
        env!("CARGO_PKG_VERSION"),
        "--json",
    ]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(
        fixture.registry()["targets"][0]["managed_versions"]["stado"],
        env!("CARGO_PKG_VERSION")
    );
    assert_eq!(
        fixture.registry()["targets"][0]["desired_versions"]["stado"],
        "88.88.88"
    );
}

#[test]
fn an_undeclared_host_is_undeclared_and_never_drift() {
    let fixture = Fixture::new(json!({}), release_platform());
    let output = fixture.stado(&["release", "host-state", "--host", HOST, "--json"]);
    assert!(output.status.success(), "{}", stderr(&output));
    let report = json_stdout(&output);
    assert_eq!(report["state"], "undeclared");
    assert_eq!(report["binaries"], json!([]));
    assert!(!stdout(&output).contains("drift"));
}

#[test]
fn host_ahead_carries_all_reporter_fields_and_apply_refuses_the_downgrade() {
    let current = env!("CARGO_PKG_VERSION");
    let declared = previous_patch(current);
    let fixture = Fixture::new(json!({"stado": declared}), release_platform());
    fixture.install_running_stado();

    let report_output = fixture.stado(&["release", "host-state", "--host", HOST, "--json"]);
    assert!(
        !report_output.status.success(),
        "host-ahead must fail its gate"
    );
    let report = json_stdout(&report_output);
    let row = &report["binaries"][0];
    assert_eq!(row["verdict"], "host-ahead");
    assert_eq!(row["version"], current);
    for field in [
        "binary",
        "version",
        "root",
        "unit",
        "state",
        "attestation",
        "receipt",
    ] {
        assert!(
            row.get(field).is_some(),
            "report omitted reporter field {field}: {row}"
        );
    }
    assert_eq!(row["attestation"], "staged-match");
    assert_eq!(
        row["receipt"],
        "delivered 2026-09-06T12:00:00Z by host-release-test"
    );

    let applied = fixture.stado(&["release", "host-state", "--host", HOST, "--apply", "--json"]);
    assert!(
        !applied.status.success(),
        "apply downgraded a host-ahead binary"
    );
    let expected = format!(
        "stado: runs {current}, newer than the declared {declared} — refused to downgrade the host; move the declaration instead: stado release declare-version --host {HOST} --binary stado --version {current}"
    );
    assert!(
        stderr(&applied).contains(&expected),
        "expected exact refusal {expected:?}, got {}",
        stderr(&applied)
    );
    assert_eq!(
        fixture.registry()["targets"][0]["managed_versions"]["stado"],
        declared,
        "a refused apply must not move desired state"
    );
}

#[test]
fn apply_reads_the_release_manifest_instead_of_inventing_an_artifact() {
    let desired = next_patch(env!("CARGO_PKG_VERSION"));
    let fixture = Fixture::new(json!({"stado": desired}), release_platform());
    fixture.install_running_stado();
    let applied = fixture.stado(&["release", "host-state", "--host", HOST, "--apply", "--json"]);
    assert!(
        !applied.status.success(),
        "apply succeeded without a release manifest"
    );
    let report = json_stdout(&applied);
    let release = &report["releases"][0];
    assert_eq!(release["status"], "failed");
    assert_eq!(
        release["detail"], "STADO_API_URL is required for canonical release reads",
        "apply must require the canonical release-manifest channel"
    );
    assert_eq!(
        fixture.registry()["targets"][0]["managed_versions"]["stado"],
        desired
    );
}

#[test]
fn an_unknown_declared_platform_is_refused_before_host_contact() {
    let fixture = Fixture::new(json!({"stado": env!("CARGO_PKG_VERSION")}), "plan9-riscv");
    let output = fixture.stado(&["release", "host-state", "--host", HOST, "--json"]);
    assert!(!output.status.success());
    let expected = format!(
        "{HOST} declares release_platform \"plan9-riscv\", which cannot carry a managed release: \"plan9-riscv\" is not a published release platform; expected one of darwin-arm64, linux-amd64; set targets[].release_platform to a published platform"
    );
    assert!(
        stderr(&output).contains(&expected),
        "expected {expected:?}, got {}",
        stderr(&output)
    );
}
