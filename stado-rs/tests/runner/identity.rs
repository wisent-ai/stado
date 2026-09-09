//! The GitHub identity the fleet declares, confronted with the vault and with
//! GitHub.
//!
//! `runner credential` is the product's own reality check: it resolves the
//! declared route through Skarbiec's route table, reads the one field that
//! route names, and calls the endpoint the declaration says the lifecycle
//! needs. Nothing here is stubbed, and nothing registers a runner.
//!
//! The case exists because the declaration used to demand `admin:org` and
//! point its check at `/orgs/wisent-ai/actions/runner-groups`, an endpoint the
//! operator's credential is refused on. Every runner command then failed with
//! HTTP 403 and asked for a token nobody had agreed to create, while the
//! runners this fleet installs are repository-scoped and need repository
//! administration — which that same credential carries.
//!
//! The first case reads this machine's own vault, because that is where the
//! declared route lives and a check against a seeded table would prove
//! nothing. It reads; it writes nothing. The second drives the same command
//! against the isolated fixture, where the route is absent, and holds the
//! refusal to the sentence an operator has to act on.

use std::process::Command;

use crate::fixture::{report, stderr, Fixture};

fn credential_report() -> (std::process::Output, serde_json::Value) {
    let output = Command::new(env!("CARGO_BIN_EXE_stado"))
        .args(["runner", "credential", "--json"])
        .output()
        .expect("the built stado binary runs");
    let document = report(&output);
    (output, document)
}

#[test]
fn the_declared_github_identity_resolves_and_github_accepts_it() {
    let (output, document) = credential_report();

    assert!(
        output.status.success(),
        "GitHub refused the declared identity: {document} {}",
        stderr(&output)
    );
    assert_eq!(
        document["status"], 200,
        "the endpoint the declaration names answered: {document}"
    );
    assert_eq!(
        document["accepted"], true,
        "the declared identity is accepted where the lifecycle uses it: {document}"
    );
}

#[test]
fn a_route_no_vault_answers_names_the_declaration_and_the_command_that_declares_it() {
    let fixture = Fixture::new();
    let output = fixture.stado(&["runner", "credential", "--json"]);
    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));

    let refusal = report(&output)["message"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        refusal.contains("github:org-runner-admin"),
        "the refusal names the route it asked for: {refusal}"
    );
    assert!(
        refusal.contains("stado-rs/data/github-identity.json"),
        "the refusal names the declaration that asked: {refusal}"
    );
    assert!(
        refusal.contains("skarbiec routes add --resource"),
        "the refusal names Skarbiec's own command, not one it has never had: {refusal}"
    );
}
