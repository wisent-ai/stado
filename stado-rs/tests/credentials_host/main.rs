//! Host credential operations against a real broker on an isolated vault.
//!
//! The area deleted on 2026-09-08 by `wisent-ai/stado#566` drove the binary
//! against a tempdir, a seeded registry and a shell script pretending to be
//! `skarbiec`, and asserted that a declaration parsed and a refusal sentence
//! was exact. Nothing in it ever reached a vault, so nothing in it was
//! evidence that Stado can put a credential where a host will read it.
//!
//! Here every case drives the built `stado` binary (`CARGO_BIN_EXE_stado`)
//! against the real Skarbiec broker built from `origin/main`, over a vault
//! this area creates and initialises with real GnuPG keys, on this machine
//! named in a registry of its own. Assertions read the persisted encrypted
//! record on disk and the exit status; stdout is corroboration, never the
//! only witness. See `host.rs` for the isolation, `broker.rs` for the broker.
//!
//! What is defended: a credential declared through the product is really in
//! the host's declared vault and the declaration reads back from persisted
//! state; the inspection verb reports what the host holds — kind, schema,
//! state, revision, and per field its length and digest — and never a value;
//! and in `refusals.rs`, the refusals that matter, each with its exact
//! sentence and with the vault left untouched.

mod broker;
mod host;
mod refusals;

use std::process::Output;

use serde_json::Value;
use sha2::{Digest, Sha256};

use host::{said, stderr, stdout, IsolatedHost, OWNER, TARGET};

/// The item these cases write. The `:` separators are the shape every real
/// Stado credential id has, and the reason `vault_word` allows them.
pub const ITEM: &str = "brama:agent:credentials-area";
pub const USERNAME: &str = "credentials-area-operator";
/// A value that exists only inside one tempdir vault for the length of one
/// test. It is never a real secret and never reaches a real vault.
pub const PASSWORD: &str = "isolated-area-value-4d81c7";

fn payload() -> String {
    serde_json::json!({
        "schema": "skarbiec.item.v2",
        "kind": "login",
        "fields": {"username": USERNAME, "password": PASSWORD},
        "context": {},
    })
    .to_string()
}

fn digest(value: &str) -> String {
    format!("{:x}", Sha256::digest(value.as_bytes()))
}

pub fn report(output: &Output) -> Value {
    serde_json::from_str(&stdout(output))
        .unwrap_or_else(|error| panic!("the report is not JSON: {error}\n{}", said(output)))
}

/// Store the item through the product and return its report.
pub fn put(host: &IsolatedHost) -> Value {
    let stored = host.run(
        &[
            "credentials",
            "item",
            "put",
            "--host",
            TARGET,
            ITEM,
            "--type",
            "login",
            "--json",
        ],
        Some(&payload()),
    );
    assert!(
        stored.status.success(),
        "the product could not store an item in the isolated vault\n{}",
        said(&stored)
    );
    report(&stored)
}

#[test]
fn a_declared_credential_reaches_the_real_vault_and_the_declaration_reads_back() {
    let host = IsolatedHost::new(true);
    let stored = put(&host);
    assert_eq!(stored["before"]["state"], "absent", "{stored:#}");
    assert_eq!(stored["after"]["state"], "active", "{stored:#}");
    assert_eq!(stored["after"]["revision"], "1", "{stored:#}");

    // The state that decides whether this worked: the encrypted record the
    // real broker wrote into the declared vault file.
    let vault = host.vault_document();
    let record = &vault["items"][ITEM];
    assert_eq!(record["state"], "active", "{vault:#}");
    assert_eq!(record["revision"], 1, "{vault:#}");
    assert_eq!(record["kind"], "login", "{vault:#}");
    assert_eq!(vault["owner"], OWNER, "{vault:#}");
    let ciphertext = record["current"]["ciphertext"]
        .as_str()
        .unwrap_or_else(|| panic!("the record holds no ciphertext\n{vault:#}"));
    assert!(
        ciphertext.starts_with("-----BEGIN PGP MESSAGE-----"),
        "the value was not encrypted at rest: {ciphertext}"
    );
    assert!(
        !String::from_utf8_lossy(&host.vault_bytes()).contains(PASSWORD),
        "the value is on disk in the clear"
    );

    // And the declaration itself, read back off the host through the product.
    let vaults = host.run(&["credentials", "vaults", "--host", TARGET, "--json"], None);
    assert!(vaults.status.success(), "{}", said(&vaults));
    let answer = report(&vaults);
    let reported = &answer["hosts"][0];
    assert_eq!(reported["target"], TARGET, "{answer:#}");
    assert_eq!(reported["authority"]["state"], "declared", "{answer:#}");
    assert_eq!(
        reported["authority"]["path"].as_str(),
        host.vault_path().to_str(),
        "{answer:#}"
    );
    assert_eq!(reported["vaults"][0]["owner"], OWNER, "{answer:#}");
    assert_eq!(reported["vaults"][0]["items"], 1, "{answer:#}");
}

#[test]
fn the_inspection_verb_reports_what_the_host_holds_and_never_the_value() {
    let host = IsolatedHost::new(true);
    put(&host);

    let shown = host.run(
        &[
            "credentials",
            "item",
            "show",
            "--host",
            TARGET,
            ITEM,
            "--json",
        ],
        None,
    );
    assert!(shown.status.success(), "{}", said(&shown));
    let answer = report(&shown);
    assert_eq!(answer["target"], TARGET, "{answer:#}");
    assert_eq!(answer["state"], "active", "{answer:#}");
    assert_eq!(answer["revision"], "1", "{answer:#}");
    assert_eq!(answer["kind"], "login", "{answer:#}");
    assert_eq!(answer["schema"], "skarbiec.item.v2", "{answer:#}");

    // The digests are computed on the host from the decrypted item, so they
    // are only these values if the command really opened this vault.
    let fields = answer["fields"]
        .as_array()
        .unwrap_or_else(|| panic!("no field report\n{answer:#}"));
    let mut seen: Vec<(&str, u64, &str)> = fields
        .iter()
        .map(|field| {
            (
                field["name"].as_str().unwrap_or_default(),
                field["length"].as_u64().unwrap_or_default(),
                field["sha256"].as_str().unwrap_or_default(),
            )
        })
        .collect();
    seen.sort();
    let password = digest(PASSWORD);
    let username = digest(USERNAME);
    assert_eq!(
        seen,
        vec![
            ("password", PASSWORD.len() as u64, password.as_str()),
            ("username", USERNAME.len() as u64, username.as_str()),
        ],
        "{answer:#}"
    );

    // The whole point of reporting a digest: the value stays on the host.
    for stream in [stdout(&shown), stderr(&shown)] {
        assert!(!stream.contains(PASSWORD), "the value leaked: {stream}");
        assert!(
            !stream.contains(USERNAME),
            "the username value leaked: {stream}"
        );
    }
}
