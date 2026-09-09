//! Installing the publisher profile on the dedicated macOS host, which issues
//! a Developer ID for the repository and reconciles that repository's signing
//! secrets.
//!
//! Issuing the certificate requires an Account Holder answering an Apple
//! two-factor prompt on that machine, which is why this is the one publisher
//! case that does not run by default. Its reason names the host, the
//! repository, the `gh` login and the consent it needs, and the exact command
//! that runs it.

use std::process::{Command, Output};

use serde_json::Value;

use crate::runner::{said, PROFILE};

/// Variables the case needs, named here so its reason and its body agree.
const TARGET_VARIABLE: &str = "STADO_PUBLISHER_TEST_TARGET";
const REPOSITORY_VARIABLE: &str = "STADO_PUBLISHER_TEST_REPOSITORY";

/// The secrets a signing repository must hold once the bootstrap has run.
const SIGNING_SECRETS: &[&str] = &[
    "MACOS_CERT_P12",
    "MACOS_CERT_PASSWORD",
    "MACOS_SIGN_IDENTITY",
];

#[test]
#[ignore = "issues a Developer ID on a dedicated macOS host: needs \
            STADO_PUBLISHER_TEST_TARGET naming that registered host, \
            STADO_PUBLISHER_TEST_REPOSITORY naming a disposable wisent-ai repository, a `gh` login \
            with admin on it, and an Account Holder answering the Apple two-factor prompt on that \
            machine. Run it with: STADO_PUBLISHER_TEST_TARGET=<host> \
            STADO_PUBLISHER_TEST_REPOSITORY=<repo> cargo test --test publisher -- --ignored \
            publisher_install_issues_the_developer_id_once_and_grants_repository_signing"]
fn publisher_install_issues_the_developer_id_once_and_grants_repository_signing() {
    let target = required(TARGET_VARIABLE);
    let repository = required(REPOSITORY_VARIABLE);
    let install = [
        "runner",
        "install",
        &target,
        "--profile",
        PROFILE,
        "--repository",
        &repository,
        "--json",
    ];

    let installed = success(&fleet(&install));
    assert_eq!(installed["status"], "completed");
    assert_eq!(installed["runner_kind"], PROFILE);
    let issued = &installed["repository_bootstrap"]["developer_id"];
    assert!(
        matches!(issued["status"].as_str(), Some("issued" | "reused")),
        "{installed}"
    );
    assert!(
        issued["identity"]
            .as_str()
            .unwrap_or_default()
            .starts_with("Developer ID Application:"),
        "{installed}"
    );
    assert_eq!(issued["repositories"][0], repository.as_str());

    // A second install must reuse the bundle rather than issue a second
    // certificate: that is what makes the prompt a one-time cost.
    let reinstalled = success(&fleet(&install));
    let reused = &reinstalled["repository_bootstrap"]["developer_id"];
    assert_eq!(reused["status"], "reused", "{reinstalled}");
    assert_eq!(reused["identity"], issued["identity"]);

    let listed = Command::new("gh")
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
        listed.status.success(),
        "gh secret list failed: {}",
        String::from_utf8_lossy(&listed.stderr)
    );
    let names: Value = serde_json::from_slice(&listed.stdout).expect("gh returns JSON");
    for wanted in SIGNING_SECRETS {
        assert!(
            names
                .as_array()
                .expect("gh returns an array")
                .iter()
                .any(|row| row["name"] == *wanted),
            "the repository is missing {wanted}: {names}"
        );
    }
}

fn required(name: &str) -> String {
    std::env::var(name)
        .unwrap_or_else(|_| panic!("{name} must name the dedicated publisher fixture"))
}

/// The built binary with the operator's own registry and configuration, which
/// the real host journey needs: the host key is brokered through them.
fn fleet(arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_stado"))
        .args(arguments)
        .env("NO_COLOR", "1")
        .output()
        .expect("stado starts")
}

fn success(output: &Output) -> Value {
    assert!(output.status.success(), "{}", said(output));
    serde_json::from_slice(&output.stdout).expect("stado returns JSON")
}
