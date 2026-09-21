//! Stamping the owner vault's payload fingerprints, against the same real
//! broker and isolated vault the other cases use.
//!
//! The pass has to run where the owner key and the canonical vault are. On
//! 2026-09-21 it was run on a laptop holding a replica: 658 items were
//! stamped, the owner's next sync replaced the file, and the duplicate report
//! was blind again within the hour — the same shape as a grant written by
//! hand to a replica. These cases drive the product's own route, so the pass
//! happens on the host that answers for the vault.

use serde_json::Value;

use crate::host::{said, IsolatedHost, TARGET};
use crate::{put, report, ITEM};

/// Puts every item back into the state a vault written before Skarbiec 0.3.11
/// holds: no payload fingerprint anywhere.
fn strip_fingerprints(host: &IsolatedHost) {
    let text = std::fs::read_to_string(host.vault_path()).expect("read the isolated vault");
    let mut document: Value = serde_json::from_str(&text).expect("the vault is JSON");
    for entry in document["items"]
        .as_object_mut()
        .expect("the vault carries items")
        .values_mut()
    {
        entry
            .as_object_mut()
            .expect("an item is an object")
            .remove("payload_fingerprint");
    }
    let encoded = serde_json::to_string_pretty(&document).expect("serialize the vault");
    std::fs::write(host.vault_path(), format!("{encoded}\n")).expect("write the isolated vault");
}

fn stamp(host: &IsolatedHost, apply: bool) -> std::process::Output {
    let mut arguments = vec![
        "credentials",
        "item",
        "stamp-fingerprints",
        "--host",
        TARGET,
        "--json",
    ];
    if apply {
        arguments.push("--apply");
    }
    host.run(&arguments, None)
}

#[test]
fn the_dry_pass_reports_what_it_would_stamp_and_writes_nothing() {
    let host = IsolatedHost::new(true);
    put(&host);
    strip_fingerprints(&host);
    let before = host.vault_bytes();

    let dry = stamp(&host, false);
    assert!(dry.status.success(), "{}", said(&dry));
    let said_dry = report(&dry);
    assert_eq!(said_dry["applied"], false, "{said_dry:#}");
    assert_eq!(said_dry["before"]["compared"], 0, "{said_dry:#}");
    assert_eq!(said_dry["pass"]["stamped"], 1, "{said_dry:#}");
    assert_eq!(
        host.vault_bytes(),
        before,
        "a dry pass wrote to the owner vault"
    );
}

#[test]
fn the_applied_pass_makes_every_row_comparable_on_the_host_that_owns_the_vault() {
    let host = IsolatedHost::new(true);
    put(&host);
    strip_fingerprints(&host);

    let applied = stamp(&host, true);
    assert!(applied.status.success(), "{}", said(&applied));
    let said_applied = report(&applied);
    assert_eq!(said_applied["applied"], true, "{said_applied:#}");
    assert_eq!(said_applied["pass"]["stamped"], 1, "{said_applied:#}");
    assert_eq!(
        said_applied["pass"]["unreadable"].as_array().map(Vec::len),
        Some(0),
        "{said_applied:#}"
    );
    assert_eq!(said_applied["after"]["compared"], 1, "{said_applied:#}");
    assert_eq!(said_applied["after"]["without_fingerprint"], 0);

    // The vault itself carries the field now, which is what survives the next
    // sync to every replica.
    let text = std::fs::read_to_string(host.vault_path()).expect("read the isolated vault");
    let document: Value = serde_json::from_str(&text).expect("the vault is JSON");
    assert!(
        document["items"][ITEM]["payload_fingerprint"].is_string(),
        "the stored item carries no fingerprint after the pass"
    );

    // A second pass has nothing left to do.
    let again = report(&stamp(&host, true));
    assert_eq!(again["pass"]["stamped"], 0, "{again:#}");
    assert_eq!(again["pass"]["already_stamped"], 1, "{again:#}");
}

#[test]
fn two_rows_holding_one_payload_report_as_a_duplicate_group_once_stamped() {
    let host = IsolatedHost::new(true);
    put(&host);
    // The write funnel refuses a second holder of one payload, so the copy is
    // made the way an old vault holds it: both rows without a fingerprint.
    let text = std::fs::read_to_string(host.vault_path()).expect("read the isolated vault");
    let mut document: Value = serde_json::from_str(&text).expect("the vault is JSON");
    let items = document["items"]
        .as_object_mut()
        .expect("the vault carries items");
    let mut copy = items
        .get(ITEM)
        .cloned()
        .expect("the stored item is in the vault");
    copy.as_object_mut()
        .expect("an item is an object")
        .remove("payload_fingerprint");
    items.insert(format!("{ITEM}-second-row"), copy);
    for entry in items.values_mut() {
        entry
            .as_object_mut()
            .expect("an item is an object")
            .remove("payload_fingerprint");
    }
    let encoded = serde_json::to_string_pretty(&document).expect("serialize the vault");
    std::fs::write(host.vault_path(), format!("{encoded}\n")).expect("write the isolated vault");

    let applied = report(&stamp(&host, true));
    assert_eq!(applied["before"]["groups"], 0, "{applied:#}");
    assert_eq!(applied["after"]["groups"], 1, "{applied:#}");
    assert_eq!(applied["after"]["items"], 2, "{applied:#}");
}
