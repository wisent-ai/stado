//! Real declared publisher-profile and Developer ID journeys, and the real
//! resolution of the GitHub identity every runner profile registers with.
//!
//! Nothing here is substituted with a local fixture. The route cases drive the
//! real `skarbiec` broker over an isolated vault, and standing that up is what
//! proved the capability cannot resolve against the broker the fleet runs.
use std::path::Path;
use std::process::{Command, Output};

#[path = "../support/skarbiec.rs"]
mod skarbiec_support;

use skarbiec_support::{
    real_skarbiec_binary, require_route_resolution, SkarbiecFixture, SkarbiecItem,
};

fn required(name: &str) -> String {
    std::env::var(name)
        .unwrap_or_else(|_| panic!("{name} must name the dedicated publisher fixture"))
}

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_stado"))
        .args(args)
        .output()
        .expect("stado starts")
}

fn success(output: &Output) -> serde_json::Value {
    assert!(
        output.status.success(),
        "stado failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    serde_json::from_slice(&output.stdout).expect("stado returns JSON")
}

#[test]
#[ignore = "Probierz supplies a dedicated macOS host and disposable GitHub repository"]
fn publisher_runner_install_reconciles_an_existing_runner_without_org_admin_access() {
    let target = required("STADO_PUBLISHER_TEST_TARGET");
    let repository = required("STADO_PUBLISHER_TEST_REPOSITORY");

    let installed = success(&run(&[
        "runner",
        "install",
        &target,
        "--profile",
        "publisher",
        "--repository",
        &repository,
        "--json",
    ]));
    assert_eq!(installed["status"], "completed");
    assert_eq!(installed["runner_kind"], "publisher");
    assert_eq!(installed["runner_group"], "Default");
    assert!(installed["stdout"]
        .as_str()
        .unwrap_or_default()
        .contains("runner service: running"));

    let status = success(&run(&[
        "runner",
        "status",
        &target,
        "--profile",
        "publisher",
        "--json",
    ]));
    assert_eq!(status["status"], "completed");
    assert_eq!(status["runner_kind"], "publisher");
}

#[test]
#[ignore = "Probierz supplies Account Holder 2FA consent on the dedicated macOS host"]
fn publisher_install_issues_once_reuses_the_bundle_and_grants_repository_signing() {
    let target = required("STADO_PUBLISHER_TEST_TARGET");
    let repository = required("STADO_PUBLISHER_TEST_REPOSITORY");

    let installed = success(&run(&[
        "runner",
        "install",
        &target,
        "--profile",
        "publisher",
        "--repository",
        &repository,
        "--json",
    ]));
    let issued = &installed["repository_bootstrap"]["developer_id"];
    assert!(matches!(
        issued["status"].as_str(),
        Some("issued" | "reused")
    ));
    assert!(issued["identity"]
        .as_str()
        .unwrap_or_default()
        .starts_with("Developer ID Application:"));
    assert_eq!(issued["repositories"][0], repository);

    let installed = success(&run(&[
        "runner",
        "install",
        &target,
        "--profile",
        "publisher",
        "--repository",
        &repository,
        "--json",
    ]));
    let reused = &installed["repository_bootstrap"]["developer_id"];
    assert_eq!(reused["status"], "reused");
    assert_eq!(reused["identity"], issued["identity"]);

    let secret_names = Command::new("gh")
        .args([
            "secret",
            "list",
            "--repo",
            &format!("wisent-ai/{repository}"),
            "--json",
            "name",
        ])
        .output()
        .expect("gh starts");
    assert!(
        secret_names.status.success(),
        "gh secret list failed: {}",
        String::from_utf8_lossy(&secret_names.stderr)
    );
    let names: serde_json::Value =
        serde_json::from_slice(&secret_names.stdout).expect("gh returns JSON");
    for required in [
        "MACOS_CERT_P12",
        "MACOS_CERT_PASSWORD",
        "MACOS_SIGN_IDENTITY",
    ] {
        assert!(
            names
                .as_array()
                .unwrap()
                .iter()
                .any(|row| row["name"] == required),
            "missing {required}"
        );
    }
}

/// The coordinate the declared route is pointed at. Deliberately nothing like
/// `GITHUB_TOKEN`, the id compiled in until 2026-09-07: a refusal naming this
/// can only have come from the route.
const FIXTURE_ITEM: &str = "publisher-fixture-github-identity";
const FIXTURE_FIELD: &str = "api_key";

/// One isolated vault, one real broker, and optionally the declared route.
///
/// The grant is real, so a read Stado is not entitled to is refused by
/// Skarbiec rather than waved through by a fixture.
fn broker(home: &Path, declare: bool) -> SkarbiecFixture {
    let route = stado::github_identity::declared()
        .expect("the shipped declaration parses")
        .credential_route
        .clone();
    let binary = real_skarbiec_binary();
    let fixture = SkarbiecFixture::start(
        home,
        &[SkarbiecItem::new(
            FIXTURE_ITEM,
            "stado-secret",
            serde_json::json!({
                "kind": "stado-secret",
                "schema": "skarbiec.item.v2",
                "context": {"source_kind": "publisher-fixture"},
                "fields": {FIXTURE_FIELD: "not-a-real-github-credential"},
            }),
        )],
        home.join("admin-token"),
        Some((
            "local-operator",
            &format!("read:{FIXTURE_ITEM}#{FIXTURE_FIELD}"),
        )),
        |gnupg, vault| {
            if !declare {
                return;
            }
            let declared = Command::new(&binary)
                .args(["route", "declare", "--resource", &route])
                .args(["--item", FIXTURE_ITEM, "--field", FIXTURE_FIELD])
                .args(["--reason", "publisher route fixture"])
                .env_clear()
                .env("HOME", home)
                .env("GNUPGHOME", gnupg)
                .env("PATH", std::env::var_os("PATH").unwrap_or_default())
                .env("SKARBIEC_VAULT_FILE", vault)
                .env("SKARBIEC_AUDIT_FILE", home.join("audit.jsonl"))
                .output()
                .expect("the real skarbiec binary runs");
            assert!(
                declared.status.success(),
                "real Skarbiec refused the route declaration: {}",
                String::from_utf8_lossy(&declared.stderr)
            );
        },
    );
    require_route_resolution(&fixture.url());
    fixture
}

/// `stado runner credential --json` against one real broker.
fn credential_report(home: &Path, fixture: &SkarbiecFixture) -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_stado"))
        .args(["runner", "credential", "--json"])
        .env("HOME", home)
        .env("STADO_CONFIG", home.join("no-such-config.json"))
        .env("STADO_CREDENTIALS_ADMIN_URL", fixture.url())
        .env("STADO_CREDENTIALS_ADMIN_CONSUMER", "local-operator")
        .env("STADO_CREDENTIALS_ADMIN_TOKEN_FILE", &fixture.token)
        .output()
        .expect("stado starts");
    assert!(!output.status.success(), "this route must refuse");
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

/// The shipped declaration is readable and states a route Skarbiec's own
/// `route declare` accepts. A coordinate such as `GITHUB_TOKEN#value` would put
/// the hardcoded id back under a new name, so the validator refuses one.
#[test]
fn the_shipped_github_identity_declaration_states_a_declarable_route() {
    let identity = stado::github_identity::declared().expect("the shipped declaration parses");
    assert!(
        identity.credential_route.contains(':') && !identity.credential_route.contains('#'),
        "credential_route is not a declarable route: {}",
        identity.credential_route
    );
    for prefix in ["provider:", "agent:", "login:"] {
        assert!(
            !identity.credential_route.starts_with(prefix),
            "credential_route claims a namespace items declare for themselves: {}",
            identity.credential_route
        );
    }
    assert!(
        identity.reality_check.starts_with('/') && !identity.required_permission.is_empty(),
        "the declaration carries no usable reality check"
    );
}

/// A route the real vault does not declare is refused by name, with the
/// declaration that asked for it and the command that declares it. The
/// predecessor could only fail as `credential GITHUB_TOKEN.value is required`,
/// naming no route and no way to point Stado at another credential.
#[test]
fn a_github_route_nothing_declares_is_refused_by_name() {
    let home = tempfile::tempdir().expect("a temp home");
    let fixture = broker(home.path(), false);
    let text = credential_report(home.path(), &fixture);
    for needle in [
        "github:org-runner-admin",
        "stado-rs/data/github-identity.json",
        "skarbiec route declare",
    ] {
        assert!(text.contains(needle), "refusal omits {needle:?}: {text}");
    }
}

/// Which coordinate Stado reads comes from the route, not from the binary.
///
/// The real broker resolves the declared route to this fixture's item and
/// field and Stado reads that field through its ordinary credential read, so
/// the refusal has to name the coordinate the route gave it — and it must
/// never name the id this declaration replaced.
#[test]
fn the_declared_github_route_decides_which_coordinate_stado_reads() {
    let home = tempfile::tempdir().expect("a temp home");
    let fixture = broker(home.path(), true);
    let text = credential_report(home.path(), &fixture);
    assert!(
        text.contains(&format!("{FIXTURE_ITEM}.{FIXTURE_FIELD}")),
        "refusal does not name the coordinate the route gave it: {text}"
    );
    assert!(
        !text.contains("GITHUB_TOKEN"),
        "a route-resolved read named the replaced item id: {text}"
    );
}
