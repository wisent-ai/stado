//! The refusals a host credential read owes, against the same real broker and
//! the same isolated vault the success cases use.
//!
//! Split from `main.rs` only because a file here is capped at three hundred
//! lines. Each sentence below was copied from a live run of the built binary,
//! not composed from the source, and each case also proves that a refused
//! read leaves the persisted vault byte-identical: a command that refuses
//! after writing is worse than one that never ran.

use crate::host::{said, stderr, IsolatedHost, TARGET};
use crate::{put, ITEM};

/// An id no case ever writes, so its absence is a property of the vault
/// rather than of the order the tests happened to run in.
const NEVER_WRITTEN: &str = "brama:agent:never-written";

#[test]
fn an_item_the_isolated_vault_does_not_hold_is_refused_by_its_sentence() {
    let host = IsolatedHost::new(true);
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
    assert!(
        stderr(&absent).contains(&format!(
            "{TARGET} declares no credential item {NEVER_WRITTEN}; add it to the vault declared \
             by secrets.skarbiec.vault_file"
        )),
        "{}",
        said(&absent)
    );
    assert_eq!(
        host.vault_bytes(),
        before,
        "a refused read wrote to the vault"
    );
}

#[test]
fn a_field_the_item_does_not_carry_is_refused_by_its_sentence() {
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
    assert!(
        stderr(&missing).contains(&format!(
            "{TARGET}: {ITEM} could not be read: the item holds no field totp_secret"
        )),
        "{}",
        said(&missing)
    );
    assert_eq!(
        host.vault_bytes(),
        before,
        "a refused read wrote to the vault"
    );
}

#[test]
fn an_unknown_host_and_a_malformed_item_name_are_refused_by_their_sentences() {
    let host = IsolatedHost::new(true);
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
    assert!(
        stderr(&unknown)
            .contains("target 'no-such-registry-host' is not in the canonical registry"),
        "{}",
        said(&unknown)
    );

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
    assert!(
        stderr(&malformed)
            .contains("vault item must contain only letters, digits, '.', '_', '-' or ':'"),
        "{}",
        said(&malformed)
    );
    assert_eq!(
        host.vault_bytes(),
        before,
        "a refused read wrote to the vault"
    );
}

#[test]
fn a_host_that_declares_no_vault_authority_is_refused_by_its_sentence() {
    // The vault exists on this host; nothing declares it. A plausible default
    // here is exactly how two vaults on one machine both receive real writes.
    let host = IsolatedHost::new(false);
    let before = host.vault_bytes();
    let refused = host.run(
        &["credentials", "item", "show", "--host", TARGET, ITEM],
        None,
    );
    assert_eq!(refused.status.code(), Some(1), "{}", said(&refused));
    assert!(
        stderr(&refused).contains(&format!(
            "{TARGET} declares no vault authority; add it to secrets.skarbiec.vault_file"
        )),
        "{}",
        said(&refused)
    );
    assert_eq!(
        host.vault_bytes(),
        before,
        "a refused read wrote to the vault"
    );
}
