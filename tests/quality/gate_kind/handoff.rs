//! `stado release changes submit` records source and builds nothing: it runs
//! the format-kind gates on the handed-off commit and writes the ticket,
//! leaving every lint and test gate to the batch build that takes the change.
//!
//! The product's lint gate compiles source that cannot compile, so a hand-off
//! that ran it would be refused; the hand-off is accepted and its ticket is
//! listed. A commit whose source the format gate refuses is not handed off
//! and no ticket names it. The checkout is pushed to an origin of its own,
//! since a hand-off names a commit on origin's main.

use super::super::{git_output, Deployment};
use serde_json::{json, Value};

/// The Oko work the journey's hand-offs name, in the task form a pending
/// change requires (`task-` and sixteen hex digits); no Oko is asked about it.
const WORK: &str = "task-0a11de5c0ffee000";
/// The session the journey's hand-offs name.
const SESSION: &str = "handoff-journey-session";
/// A checkout with the declared gates, committed and pushed to its origin;
/// its path and the commit handed off.
fn pushed(deployment: &mut Deployment, name: &str, source: &str) -> (String, String) {
    let gates = json!([
        {"name": "style", "kind": "format", "argv": ["rustfmt", "--check", "src/lib.rs"]},
        {"name": "compile", "kind": "lint", "argv": ["rustc", "--crate-type", "lib", "src/lib.rs"]}
    ]);
    let checkout = deployment.checkout(name, source, gates);
    let origin = deployment.root.join(format!("{name}-origin.git"));
    std::fs::create_dir_all(&origin).unwrap();
    deployment.git(
        &origin,
        &["init", "--quiet", "--bare", "--initial-branch=main"],
    );
    let checkout_path = std::path::PathBuf::from(&checkout);
    deployment.git(
        &checkout_path,
        &["remote", "add", "origin", &origin.to_string_lossy()],
    );
    deployment.git(&checkout_path, &["push", "--quiet", "origin", "main"]);
    let report = deployment.root.join("report.json");
    let commit = git_output(
        &checkout_path,
        &["rev-parse", "HEAD"],
        &mut deployment.report,
        &report,
    );
    (checkout, commit)
}

/// The tickets `stado release changes list --json` holds for `commit`.
fn tickets(deployment: &mut Deployment, commit: &str) -> Vec<Value> {
    let listed = deployment.run(&["release", "changes", "list", "--json"]);
    assert!(
        listed.status.success(),
        "{}",
        String::from_utf8_lossy(&listed.stderr)
    );
    let rows: Value = serde_json::from_slice(&listed.stdout).expect("the change list is JSON");
    let rows = rows
        .as_array()
        .cloned()
        .expect("the change list is an array");
    rows.into_iter()
        .filter(|row| row["source_commit"] == commit)
        .collect()
}

#[test]
fn a_hand_off_records_source_without_running_the_gates_that_build() {
    let mut deployment = Deployment::start();
    let initialised = deployment.run(&["config", "init"]);
    assert!(
        initialised.status.success(),
        "{}",
        String::from_utf8_lossy(&initialised.stderr)
    );

    let (checkout, commit) = pushed(
        &mut deployment,
        "handed-off",
        "pub fn answer() -> bool {\n    missing_answer\n}\n",
    );
    let handed = deployment.run(&[
        "release",
        "changes",
        "submit",
        "--source",
        &checkout,
        "--commit",
        &commit,
        "--task",
        WORK,
        "--session",
        SESSION,
        "--json",
    ]);
    let said = format!(
        "{}{}",
        String::from_utf8_lossy(&handed.stdout),
        String::from_utf8_lossy(&handed.stderr)
    );
    assert!(
        handed.status.success(),
        "a formatted commit was not handed off: {said}"
    );
    assert!(
        !said.contains("rustc"),
        "the hand-off ran the lint gate that builds: {said}"
    );
    let ticket: Value =
        serde_json::from_slice(&handed.stdout).expect("the hand-off answers its ticket");
    let kept = tickets(&mut deployment, &commit);
    assert!(
        kept.iter().any(|row| row["id"] == ticket["id"]),
        "the ticket was not kept: {said}"
    );

    let (unformatted, refused_commit) = pushed(
        &mut deployment,
        "unformatted",
        "pub fn answer()->bool{true}\n",
    );
    let refused = deployment.run(&[
        "release",
        "changes",
        "submit",
        "--source",
        &unformatted,
        "--commit",
        &refused_commit,
        "--task",
        WORK,
        "--session",
        SESSION,
        "--json",
    ]);
    let said = String::from_utf8_lossy(&refused.stderr).into_owned();
    assert!(
        !refused.status.success(),
        "an unformatted commit was handed off: {said}"
    );
    assert!(
        said.contains("is not handed off") && said.contains("style"),
        "{said}"
    );
    assert!(
        tickets(&mut deployment, &refused_commit).is_empty(),
        "a refused hand-off left a ticket: {said}"
    );
}
