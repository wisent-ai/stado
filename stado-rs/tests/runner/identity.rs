//! Read the declared credential through the real Skarbiec service and exercise
//! its GitHub permission. A local registry fixture does not isolate Skarbiec:
//! absence in that fixture cannot prove that a route is absent in the vault.

use std::process::Command;

use crate::fixture::{report, stderr};

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
