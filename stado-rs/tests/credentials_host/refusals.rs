//! The refusals a host credential read owes, against the same real broker and
//! the same isolated vault the success cases use.
//!
//! A refused operation must leave the real vault unchanged. The counterexamples
//! hold usable data, so falling back to another item, host or vault would succeed
//! and fail these checks. Error wording is not a contract here.

use crate::host::{said, IsolatedHost, TARGET};
use crate::{put, ITEM};

/// An id no case ever writes, so its absence is a property of the vault
/// rather than of the order the tests happened to run in.
const NEVER_WRITTEN: &str = "brama:agent:never-written";

#[test]
fn an_unknown_item_cannot_return_another_stored_item() {
    let host = IsolatedHost::new(true);
    put(&host);
    let before = host.vault_bytes();
    let absent = host.run(
        &[
            "credentials",
            "item",
            "show",
            "--host",
            TARGET,
            NEVER_WRITTEN,
            "--json",
        ],
        None,
    );
    assert_eq!(absent.status.code(), Some(1), "{}", said(&absent));
    assert_eq!(
        host.vault_bytes(),
        before,
        "a refused read wrote to the vault"
    );
}

#[test]
fn an_unknown_field_cannot_return_the_whole_item() {
    let host = IsolatedHost::new(true);
    put(&host);
    let before = host.vault_bytes();
    let missing = host.run(
        &[
            "credentials",
            "item",
            "show",
            "--host",
            TARGET,
            ITEM,
            "--field",
            "totp_secret",
        ],
        None,
    );
    assert_eq!(missing.status.code(), Some(1), "{}", said(&missing));
    assert!(missing.stdout.is_empty(), "{}", said(&missing));
    assert_eq!(
        host.vault_bytes(),
        before,
        "a refused read wrote to the vault"
    );
}

#[test]
fn unknown_hosts_and_malformed_item_names_cannot_select_a_local_item() {
    let host = IsolatedHost::new(true);
    put(&host);
    let before = host.vault_bytes();

    let unknown = host.run(
        &[
            "credentials",
            "item",
            "show",
            "--host",
            "no-such-registry-host",
            ITEM,
        ],
        None,
    );
    assert_eq!(unknown.status.code(), Some(1), "{}", said(&unknown));

    // Refused before the host is contacted at all, because these words are
    // interpolated into an owner write on the host.
    let malformed = host.run(
        &[
            "credentials",
            "item",
            "show",
            "--host",
            TARGET,
            "brama agent/never-written;rm",
        ],
        None,
    );
    assert_eq!(malformed.status.code(), Some(2), "{}", said(&malformed));
    assert_eq!(
        host.vault_bytes(),
        before,
        "a refused read wrote to the vault"
    );
}

#[test]
fn withdrawing_authority_cannot_fall_back_to_a_usable_default_vault() {
    let host = IsolatedHost::new(true);
    put(&host);
    let default_vault = host.home.join(".stado/skarbiec.vault.json");
    std::fs::rename(host.vault_path(), &default_vault).unwrap();
    let withdrawn = host.run(
        &[
            "host",
            "config-set",
            TARGET,
            "secrets.skarbiec.vault_file",
            "null",
        ],
        None,
    );
    assert!(withdrawn.status.success(), "{}", said(&withdrawn));
    let before = std::fs::read(&default_vault).unwrap();
    let refused = host.run(
        &["credentials", "item", "show", "--host", TARGET, ITEM],
        None,
    );
    assert_eq!(refused.status.code(), Some(1), "{}", said(&refused));
    assert_eq!(std::fs::read(&default_vault).unwrap(), before);
}
