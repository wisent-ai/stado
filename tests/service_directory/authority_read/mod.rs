//! Every registry read of a stado whose store is an object API takes the
//! registry authority's snapshot over SSH, so a store that never answers is
//! never asked.
//!
//! The real `stado` runs with an isolated `HOME` under the checkout's build
//! directory, a last-known-good registry copy in `~/.stado/cache` that names
//! this machine as `example-reader` and a dedicated, declared fleet host
//! (the fixture's) as the service directory's registry authority, and
//! `WC_STORAGE_BACKEND=stado` pointing at a loopback listener this test
//! opens and never answers. `stado registry self` and `stado service
//! directory show --json` must answer from the authority's `stado resolver
//! snapshot` over real SSH: their stderr carries the `czekam … resolver
//! snapshot; gdzie: SSH to <authority>` line and no `czekam: GET
//! /api/object` line, the listener receives no connection, and the copy is
//! refreshed from the snapshot. A reader with no copy has no authority to
//! ask and stands on its store, saying so.
//!
//! The fixture, `STADO_REGISTRY_AUTHORITY_FIXTURE`, is an owner-only JSON
//! file under `.build`: `{"dedicated": true, "target": <registry name>,
//! "ssh": <the SSH destination the authority is reached at>, "stado_command":
//! <the authority's stado binary>, "ssh_key_file": <private key authorized
//! there>, "known_hosts_file": <OpenSSH known_hosts carrying its host key>}`.
//! No fixture is a failed run, never a passed one.
mod deployment;

use deployment::{Deployment, Fixture, READER, STORE_ASKED};
use serde_json::{json, Value};
use std::fs;
use std::io::BufRead;
use std::process::Output;

/// A read answered from the authority: its stderr says it waited on the
/// authority's snapshot over SSH and never on the store.
fn answered_from_authority(output: &Output, fixture: &Fixture, what: &str) {
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "{what} answers from the authority's snapshot: {stderr}"
    );
    let snapshot = format!(
        "czekam: {} resolver snapshot; gdzie: SSH to {}; rodzaj: host;",
        fixture.stado_command, fixture.ssh
    );
    assert!(
        stderr.contains(&snapshot),
        "{what} says it waited on the authority's snapshot over SSH: {stderr}"
    );
    assert!(
        !stderr.contains(STORE_ASKED),
        "{what} never waited on the object API store: {stderr}"
    );
}

#[test]
fn every_registry_read_takes_the_authoritys_snapshot_over_ssh_before_the_store() {
    let fixture = Fixture::read();
    let mut deployment = Deployment::start(&fixture);
    deployment.write_last_good(&fixture);
    let copy_before = fs::read_to_string(deployment.last_good()).unwrap();

    let own = deployment.stado(&fixture, &["registry", "self", "--name-only"]);
    answered_from_authority(&own, &fixture, "stado registry self");
    assert_eq!(
        String::from_utf8_lossy(&own.stdout).trim(),
        READER,
        "the authority's registry names this machine as the reader the copy declared"
    );
    assert!(
        !deployment.store_was_asked(),
        "the object API store received no connection for registry self"
    );

    let directory = deployment.stado(&fixture, &["service", "directory", "show", "--json"]);
    answered_from_authority(&directory, &fixture, "stado service directory show");
    let shown: Value = serde_json::from_slice(&directory.stdout)
        .expect("service directory show --json prints the directory");
    assert!(
        shown.get("generation").is_some() || shown.get("services").is_some(),
        "the directory shown is the authority's own: {shown}"
    );
    assert!(
        !deployment.store_was_asked(),
        "the object API store received no connection for service directory show"
    );

    let copy_after = fs::read_to_string(deployment.last_good()).unwrap();
    assert_ne!(
        copy_after, copy_before,
        "the last-known-good copy is refreshed from the authority's snapshot"
    );
    let refreshed: Value = serde_json::from_str(&copy_after).unwrap();
    assert_eq!(
        refreshed["service_directory"]["authority"]["target"],
        json!(fixture.target),
        "the refreshed copy still names the authority: {refreshed}"
    );
    deployment.finish("passed");
}

#[test]
fn a_reader_with_no_copy_stands_on_its_store_and_says_so() {
    // Without a last-known-good copy there is no authority to ask, so the
    // read goes to the store and says so before it waits; the store never
    // answers, so the command stands on it and this test ends it. The
    // fixture binds the run to the same dedicated host.
    let fixture = Fixture::read();
    let mut deployment = Deployment::start(&fixture);
    let mut child = deployment.spawn(&["registry", "self", "--name-only"]);
    let stderr = child.stderr.take().unwrap();
    let mut lines = std::io::BufReader::new(stderr).lines();
    let first_wait = loop {
        match lines.next() {
            Some(Ok(line)) if line.starts_with("czekam: ") => break line,
            Some(Ok(_)) => continue,
            Some(Err(error)) => panic!("the command's stderr could not be read: {error}"),
            None => panic!("the command ended without saying what it waited on"),
        }
    };
    assert!(
        first_wait.starts_with(STORE_ASKED),
        "with no copy the first wait is the store's own: {first_wait}"
    );
    assert!(
        deployment.store_was_asked(),
        "the store was asked, as the line says"
    );
    let _ = child.kill();
    let _ = child.wait();
    deployment.note("registry self with no copy", &first_wait);
    deployment.finish("passed");
}
