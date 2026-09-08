//! What a beacon publication does on this machine, and what it refuses.

use serde_json::Value;

use crate::fleet::{said, stderr, stdout, Fleet, TARGET};

/// The `reported_at` the fixture's own documents carry. It is the collector's
/// field, copied through the publication untouched, so its exact value is
/// what proves nothing rewrote the document on the way.
const REPORTED_AT: &str = "2026-08-19T19:00:00Z";

/// The three path answers the product's link block is allowed to give.
const PATH_KINDS: [&str; 3] = ["direct", "relay", "unknown"];

/// The slug a relayed document is about: a host this machine publishes for
/// without being it.
const RELAYED: &str = "relayed-host";

#[test]
fn a_published_beacon_lands_in_the_store_and_reads_back_through_the_product() {
    let fleet = Fleet::new();
    let document = fleet.document(&fleet.host, REPORTED_AT);

    let published = fleet.run(&["host", "publish-beacon", document.to_str().unwrap()]);
    assert!(published.status.success(), "{}", said(&published));
    // The publisher names the host it published for, and nothing else.
    assert_eq!(stdout(&published), format!("{}\n", fleet.host));

    // The object the listener itself wrote into the fleet store.
    let stored = fleet
        .stored(&fleet.host)
        .expect("the listener stored a beacon object");
    assert_eq!(stored["host"], fleet.host.as_str());
    assert_eq!(stored["reported_at"], REPORTED_AT);
    assert_eq!(
        stored["units"]["com.wisent.host-health-beacon"]["state"],
        "loaded"
    );

    // The publisher merged its own account of this machine's connectivity
    // into the document before sending it, so the stored bytes carry a block
    // the fixture never wrote.
    let link = &stored["link"];
    assert!(
        PATH_KINDS.contains(&link["path_kind"].as_str().unwrap_or_default()),
        "the stored link block names no known path kind: {link}"
    );
    let collected_at = link["collected_at"]
        .as_str()
        .expect("the collected link block dates itself");
    assert!(
        collected_at.ends_with('Z') && collected_at.len() == REPORTED_AT.len(),
        "collected_at is not a UTC second stamp: {collected_at}"
    );
    assert!(
        link["interface_changes"].is_array(),
        "the stored link block carries no interface change list: {link}"
    );

    // The same object, read back through the product's own reader rather than
    // off the disk.
    let read_back = fleet.run(&["host", "health", TARGET, "--json"]);
    assert!(read_back.status.success(), "{}", said(&read_back));
    let report: Value =
        serde_json::from_slice(&read_back.stdout).expect("the health report is JSON");
    assert_eq!(report["target"]["name"], TARGET);
    assert_eq!(report["beacon"], stored);
    let uri = report["object"]["uri"]
        .as_str()
        .expect("the report names the object it read");
    assert!(
        uri.ends_with(&format!("host_health/{}.json", fleet.host)),
        "the report read some other object: {uri}"
    );
}

#[test]
fn a_relayed_document_is_published_without_this_machines_link_block() {
    // The macOS collector relays beacons for hosts that cannot publish for
    // themselves. Stamping this machine's connectivity onto another machine's
    // document would invent the evidence the block exists to provide.
    let fleet = Fleet::new();
    let document = fleet.document(RELAYED, REPORTED_AT);

    let published = fleet.run(&["host", "publish-beacon", document.to_str().unwrap()]);
    assert!(published.status.success(), "{}", said(&published));
    assert_eq!(stdout(&published), format!("{RELAYED}\n"));

    let stored = fleet
        .stored(RELAYED)
        .expect("the listener stored the relayed beacon");
    assert_eq!(stored["host"], RELAYED);
    assert_eq!(stored["reported_at"], REPORTED_AT);
    assert_eq!(
        stored.get("link"),
        None,
        "a relayed document must carry no link block: {stored}"
    );
    // Relaying somebody else's beacon never wrote this machine's own.
    assert!(fleet.stored(&fleet.host).is_none());
}

#[test]
fn a_bearer_the_route_does_not_accept_is_refused_by_the_listener() {
    let fleet = Fleet::new();
    let document = fleet.document(&fleet.host, REPORTED_AT);
    let wrong = fleet.token_file("wrong-token", "not-the-provisioned-bearer");

    let refused = fleet.run_with_token(
        &["host", "publish-beacon", document.to_str().unwrap()],
        &wrong,
    );
    assert_eq!(refused.status.code(), Some(1), "{}", said(&refused));
    assert_eq!(
        stderr(&refused).lines().next(),
        Some(
            "Error: Stado host-health API returned HTTP 401 Unauthorized: \
             {\"error\":\"unauthorized\"}"
        )
    );
    assert!(
        fleet.stored(&fleet.host).is_none(),
        "a refused publication stored a beacon anyway"
    );
}

#[test]
fn a_document_the_publisher_cannot_read_is_refused_before_the_listener() {
    let fleet = Fleet::new();

    let malformed = fleet.write("malformed.json", "not json");
    let out = fleet.run(&["host", "publish-beacon", malformed.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(1), "{}", said(&out));
    assert_eq!(
        stderr(&out).lines().next(),
        Some("Error: host beacon is not valid JSON: expected ident at line 1 column 2")
    );

    // A document with no units is not a beacon, and no amount of link
    // collection makes it one.
    let unitless = fleet.write(
        "unitless.json",
        &format!(
            r#"{{"host": "{}", "reported_at": "{REPORTED_AT}"}}"#,
            fleet.host
        ),
    );
    let out = fleet.run(&["host", "publish-beacon", unitless.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(1), "{}", said(&out));
    assert_eq!(
        stderr(&out).lines().next(),
        Some("Error: host beacon requires string reported_at and object units fields")
    );

    let misnamed = fleet.write(
        "misnamed.json",
        &format!(
            r#"{{"host": "Beacon_Probe_Host", "reported_at": "{REPORTED_AT}", "units": {{}}}}"#
        ),
    );
    let out = fleet.run(&["host", "publish-beacon", misnamed.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(1), "{}", said(&out));
    assert_eq!(
        stderr(&out).lines().next(),
        Some("Error: host beacon host must be a lowercase DNS label")
    );

    // Nothing any of the three refusals touched reached the store.
    assert!(fleet.stored(&fleet.host).is_none());
}

#[test]
fn a_host_outside_the_registry_has_no_beacon_to_read() {
    let fleet = Fleet::new();
    let out = fleet.run(&["host", "health", "not-in-registry", "--json"]);
    assert!(!out.status.success(), "{}", said(&out));
    assert_eq!(
        stderr(&out).lines().next(),
        Some("Error: target \"not-in-registry\" is not present in the GCS registry")
    );

    // A registered host that has published nothing is a different answer from
    // a host nobody declared.
    let out = fleet.run(&["host", "health", TARGET, "--json"]);
    assert!(!out.status.success(), "{}", said(&out));
    assert!(
        stderr(&out).contains("no host health beacon for"),
        "{}",
        stderr(&out)
    );
}
