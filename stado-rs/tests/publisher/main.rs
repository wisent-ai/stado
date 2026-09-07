//! Real declared publisher-profile and Developer ID journeys.
//!
//! These tests require a dedicated macOS target and disposable repository;
//! no part of the release path is substituted with a local fixture.
use std::process::{Command, Output};

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
