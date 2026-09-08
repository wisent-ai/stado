//! What `stado host link` refuses, against this machine.
//!
//! Two refusals, both real here: a target name the isolated registry does not
//! carry, and this machine declared with no beacon document at all. Every
//! sentence asserted below was copied out of a live run of the built binary.

use serde_json::{json, Value};

use crate::fixture::{blockers, document, stderr, stdout, Fixture};
use crate::machine;

/// A name nothing in the isolated registry carries. Half a report about a
/// host that does not exist is worse than none, so the command produces no
/// document and writes nothing.
#[test]
fn a_target_the_isolated_registry_does_not_carry_is_refused() {
    let fixture = Fixture::new();

    let out = fixture.stado(&["host", "link", "no-such-host", "--json"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stderr(&out).contains("target 'no-such-host' is not in the canonical registry"),
        "got: {}",
        stderr(&out)
    );
    assert!(stdout(&out).trim().is_empty(), "got: {}", stdout(&out));
    assert!(!fixture.silence_dir().exists());
}

/// This machine is in the registry and has published nothing.
///
/// A host that has never published is not a host that is fine: the store's
/// own sentence about the missing object is a blocker, the link fields are
/// nulls rather than a fabricated path, and the gap starts at the moment of
/// observation because there is no last-heard-from instant to start it at.
#[test]
fn a_registered_host_with_no_beacon_at_all_is_reported_as_unreadable() {
    let fixture = Fixture::new();
    let host = fixture.host();

    let out = fixture.stado(&["host", "link", &host, "--json"]);
    assert_eq!(
        out.status.code(),
        Some(1),
        "no beacon is not healthy: {}",
        stderr(&out)
    );
    let report = document(&out);
    assert_eq!(report["verdict"], "degraded");
    assert_eq!(report["beacon_age_seconds"], Value::Null);
    assert_eq!(report["ssh_reachable"], true);
    assert_eq!(report["path_kind"], "unknown");
    for field in ["endpoint", "last_sleep_at", "last_wake_at"] {
        assert_eq!(report[field], Value::Null, "{field} was invented");
    }
    assert_eq!(report["interface_changes"], json!([]));

    let named = blockers(&report);
    assert!(
        named.contains(
            &"this host's beacon carries no link block, so its path, its sleep and wake times \
              and its interface changes are unknown here"
                .to_string()
        ),
        "the missing block is named in the component's own words, got: {named:?}"
    );
    // The store's own sentence names both objects it looked for, which are
    // this machine's two registry-owned identities: the slug and the full
    // host name the operating system reports here.
    let missing = format!(
        "no host health beacon for {host:?}; checked host_health/{host}.json, \
         host_health/{}.json",
        machine::hostname()
    );
    assert!(
        named.contains(&missing),
        "the store's sentence names the objects it checked, got: {named:?}"
    );

    // A host that answers while nothing has heard from it is a publisher
    // failure, so the reader reads this machine's own beacon publisher. The
    // diagnosis it produces has to be about the isolated HOME this case gave
    // it and never the operator's.
    let publisher = &report["beacon_publisher"];
    assert_eq!(publisher["unit"], "com.wisent.host-health-beacon");
    let detail = publisher["detail"]
        .as_str()
        .expect("the publisher diagnosis carries a sentence");
    assert!(
        named.contains(&detail.to_string()),
        "the publisher diagnosis reaches the blockers, got: {named:?}"
    );
    assert!(
        detail.contains(&fixture.home().display().to_string()),
        "the diagnosis read the isolated HOME, got: {detail}"
    );

    // The gap is on disk, and it starts now because nothing was ever heard.
    let records = fixture.silences();
    assert_eq!(records.len(), 1, "one open record, got: {records:?}");
    assert_eq!(records[0]["host"], host.as_str());
    assert_eq!(records[0]["ended_at"], Value::Null);
    assert_eq!(
        records[0]["first_reader_error"].as_str(),
        Some(missing.as_str()),
        "the record keeps the sentence the reader that noticed first wrote"
    );
}
