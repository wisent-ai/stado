//! `stado secrets put|get` against the real Skarbiec binary.
//!
//! The broker is resolved the way the product resolves it —
//! `SKARBIEC_TEST_BIN`, then `SKARBIEC_BIN`, then `PATH`, then
//! `~/.stado/bin/skarbiec` — and a run with none of those present refuses by
//! naming what is missing instead of passing. Every process uses an isolated
//! HOME, GnuPG home, vault, grant, storage root and loopback port, so the
//! operator's vault is never opened.

#[path = "../support/skarbiec.rs"]
mod skarbiec_support;

mod fixture;

use serde_json::Value;

use fixture::{assert_success, SkarbiecFixture, ITEM};

/// A typed credential item written by `stado secrets put` really lands in the
/// vault, with the kind it was given and both fields intact. The assertion
/// reads the item back with the Skarbiec binary itself, not from the Stado
/// output that claimed to have stored it.
#[test]
fn secrets_put_writes_a_typed_item_to_real_skarbiec() {
    let fixture = SkarbiecFixture::new();
    let put = fixture.stado(
        &["secrets", "put", ITEM, "--type", "login"],
        Some(r#"{"username":"alice","password":"not-returned"}"#),
    );
    assert_success(&put, "stado secrets put");
    assert_eq!(
        String::from_utf8_lossy(&put.stdout),
        format!("stored credential item \"{ITEM}\" as \"login\"\n")
    );

    let stored = fixture.skarbiec(&["get", ITEM]);
    assert_success(&stored, "read fixture state with Skarbiec");
    let document: Value = serde_json::from_slice(&stored.stdout).expect("stored item is JSON");
    assert_eq!(document["kind"], "login");
    assert_eq!(document["fields"]["username"], "alice");
    assert_eq!(document["fields"]["password"], "not-returned");
}

/// A grant naming one field lets that field be read and refuses the other.
///
/// This is the boundary the credential store exists to hold: a consumer that
/// can read `username` must not be able to read `password` from the same item,
/// and the refusal has to come from the broker rather than from a client that
/// chose to ask nicely.
#[test]
fn secrets_get_reads_only_the_granted_field_from_real_skarbiec() {
    let mut fixture = SkarbiecFixture::new();
    fixture.seed_login();
    fixture.grant_username();
    fixture.start_server();

    let get = fixture.stado(&["secrets", "get", ITEM, "--field", "username"], None);
    assert_success(&get, "stado secrets get");
    assert_eq!(String::from_utf8_lossy(&get.stdout), "alice\n");

    let refused = fixture.stado(&["secrets", "get", ITEM, "--field", "password"], None);
    assert!(
        !refused.status.success(),
        "reading the ungranted field succeeded: {}",
        String::from_utf8_lossy(&refused.stdout)
    );
    assert!(
        String::from_utf8_lossy(&refused.stderr)
            .contains("consumer not authorized to read item field"),
        "unexpected refusal: {}",
        String::from_utf8_lossy(&refused.stderr)
    );
}
