//! A registry write of a stado whose store is an object API runs on the
//! registry authority over SSH, so a store that never answers — on a
//! workstation, the address its own resolver serves — never holds a write.
//!
//! The same isolated deployment as the reads: the real `stado`, the dedicated
//! authority the fixture names, a last-known-good copy pointing at it and a
//! loopback store that is never answered. `stado registry set` changes the
//! reader's `notes` on the authority; its stderr carries the `czekam … registry
//! push - …; gdzie: SSH to <authority>` line and no `czekam: GET /api/object`
//! line, the store receives no connection, and `stado registry pull --path`
//! reads the new value back from the authority. The field is then set back to
//! what it held, the same way.

use super::deployment::{Deployment, Fixture, READER, STORE_ASKED};
use super::Value;

/// The reader's `notes` as the authority holds it now.
fn notes(deployment: &mut Deployment, fixture: &Fixture) -> Value {
    let path = format!("targets.{READER}.notes");
    let read = deployment.stado(fixture, &["registry", "pull", "--path", &path]);
    assert!(
        read.status.success(),
        "registry pull --path {path} answers: {}",
        String::from_utf8_lossy(&read.stderr)
    );
    serde_json::from_slice(&read.stdout).expect("registry pull --path prints the field as JSON")
}

/// Set the reader's `notes` and require that the write went to the authority
/// over SSH and never to the store.
fn set_notes(deployment: &mut Deployment, fixture: &Fixture, value: &str) {
    let path = format!("targets.{READER}.notes");
    let set = deployment.stado(
        fixture,
        &["registry", "set", "--path", &path, "--value", value],
    );
    let stderr = String::from_utf8_lossy(&set.stderr);
    assert!(set.status.success(), "registry set {path} lands: {stderr}");
    let pushed = format!(
        "czekam: {} registry push - --if-generation",
        fixture.stado_command
    );
    assert!(
        stderr.contains(&pushed),
        "the swap ran on the authority as registry push -: {stderr}"
    );
    assert!(
        stderr.contains(&format!("gdzie: SSH to {}", fixture.ssh)),
        "the swap went over SSH to the authority: {stderr}"
    );
    assert!(
        !stderr.contains(STORE_ASKED),
        "the write never waited on the object API store: {stderr}"
    );
    assert!(
        !deployment.store_was_asked(),
        "the object API store received no connection for registry set"
    );
}

#[test]
fn a_registry_write_runs_on_the_authority_over_ssh_and_never_asks_the_store() {
    let fixture = Fixture::read();
    let mut deployment = Deployment::start(&fixture);
    deployment.write_last_good(&fixture);
    let before = notes(&mut deployment, &fixture);
    let marker = format!("authority write journey {}", uuid::Uuid::new_v4());

    set_notes(&mut deployment, &fixture, &marker);
    assert_eq!(
        notes(&mut deployment, &fixture),
        Value::String(marker.clone()),
        "the authority holds the value the write set"
    );

    let restored = before
        .as_str()
        .expect("the reader's notes are a string the write can restore");
    set_notes(&mut deployment, &fixture, restored);
    assert_eq!(
        notes(&mut deployment, &fixture),
        before,
        "the authority holds the notes it held before the journey"
    );
    deployment.finish("passed");
}
