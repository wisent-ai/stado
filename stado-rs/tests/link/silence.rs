//! The silence half, against this machine.
//!
//! The fake this replaced wrote a `host_health` object and a
//! `host_silence/h1/<key>.json` record by hand and asserted the reader echoed
//! them. Here the beacon is published through the product, its `reported_at`
//! is the only thing this test chooses, and the record the reader leaves
//! under the isolated storage root is read back off disk.
//!
//! One verdict from the old area cannot happen on a current-host target and
//! is not simulated: `silent` requires the channel not to answer, and a
//! machine always answers itself. A stale beacon on a host that answers is
//! `degraded` — a box that is running and not reporting — which is the
//! verdict this machine can really produce.

use chrono::{TimeDelta, Utc};
use serde_json::{json, Value};

use crate::fixture::{
    beacon_time, blockers, document, instant, stderr, Fixture, THRESHOLD_SECONDS,
};

/// How far past the threshold the aged beacon is stamped. Three times the
/// threshold, so the age the report computes cannot land near the boundary
/// however long the real probes take.
const AGED_SECONDS: i64 = 900;

/// The reader that noticed, in the product's own vocabulary
/// (`monitor::host_silence::READER_CLI`).
const READER: &str = "cli";

#[test]
fn a_beacon_past_the_threshold_is_recorded_as_a_silence_against_this_host() {
    let fixture = Fixture::new();
    let host = fixture.host();
    let aged_at = Utc::now() - TimeDelta::seconds(AGED_SECONDS);
    let published = fixture.publish_beacon(&beacon_time(aged_at));
    fixture.seed_beacon(&published);

    let out = fixture.stado(&["host", "link", &host, "--json"]);
    assert_eq!(
        out.status.code(),
        Some(1),
        "a verdict that is not healthy exits 1: {}",
        stderr(&out)
    );
    let report = document(&out);
    // This machine answers its own channel, so nothing has heard from its
    // beacon while the box is plainly running: that is a publisher failure,
    // not a network mystery, and the product names it apart from silence.
    assert_eq!(report["verdict"], "degraded");
    assert_eq!(report["ssh_reachable"], true);
    let age = report["beacon_age_seconds"]
        .as_i64()
        .expect("beacon_age_seconds is a number");
    assert!(
        age >= AGED_SECONDS,
        "the beacon this test stamped is at least {AGED_SECONDS}s old, got {age}"
    );

    let named = blockers(&report);
    assert!(
        named.contains(&format!(
            "this host's newest beacon is {age}s old, past the {THRESHOLD_SECONDS}s silence \
             threshold"
        )),
        "the staleness blocker names the age it computed and the product's own threshold, got: \
         {named:?}"
    );
    assert!(
        stderr(&out).contains(&format!("{host} link verdict is degraded, with")),
        "got: {}",
        stderr(&out)
    );
    assert!(
        stderr(&out).contains("blocker(s) named in the report above"),
        "got: {}",
        stderr(&out)
    );

    // The gap this look noticed is now a document under the isolated root,
    // keyed by the instant the host was last heard from. That instant is the
    // one this test put in the beacon, not the instant somebody looked.
    let records = fixture.silences();
    assert_eq!(records.len(), 1, "one open record, got: {records:?}");
    let record = &records[0];
    assert_eq!(record["host"], host.as_str());
    assert_eq!(
        instant(record, "started_at").timestamp(),
        aged_at.timestamp(),
        "the gap starts when the host was last heard from"
    );
    assert_eq!(record["ended_at"], Value::Null);
    assert_eq!(record["duration_seconds"], Value::Null);
    assert_eq!(record["observed_by"], json!([READER]));
    // The record the reader wrote is the record the document carried.
    assert_eq!(report["silences"].as_array().map(Vec::len), Some(1));
    assert_eq!(report["silences"][0]["started_at"], record["started_at"]);

    // Looking twice records one gap with one observer, not two records: the
    // count an operator reads must not be a function of how often they looked.
    let out = fixture.stado(&["host", "link", &host, "--json"]);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(fixture.silences(), records);
}

/// The other half of the transition, end to end through the product: the gap
/// the previous look opened is closed by the next beacon this machine
/// publishes, at that beacon's own instant, and the duration is the outage
/// rather than the interval between two looks.
#[test]
fn the_next_beacon_this_machine_publishes_closes_the_gap_at_its_own_instant() {
    let fixture = Fixture::new();
    let host = fixture.host();
    let aged_at = Utc::now() - TimeDelta::seconds(AGED_SECONDS);
    fixture.seed_beacon(&fixture.publish_beacon(&beacon_time(aged_at)));
    assert_eq!(
        fixture
            .stado(&["host", "link", &host, "--json"])
            .status
            .code(),
        Some(1),
        "the aged beacon opens the gap"
    );
    assert_eq!(fixture.silences().len(), 1);

    // The host publishes again. This is the whole point of the record: on the
    // incident that motivated the command, the six minutes a host spent
    // unreachable were recorded nowhere once it came back.
    let back_at = Utc::now();
    fixture.seed_beacon(&fixture.publish_beacon(&beacon_time(back_at)));

    let out = fixture.stado(&["host", "link", &host, "--json"]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "the host is back and nothing else is wrong: {}",
        stderr(&out)
    );
    let report = document(&out);
    assert_eq!(report["verdict"], "healthy");

    let records = fixture.silences();
    assert_eq!(records.len(), 1, "one gap, closed, not a second record");
    let record = &records[0];
    assert_eq!(
        instant(record, "started_at").timestamp(),
        aged_at.timestamp()
    );
    assert_eq!(
        instant(record, "ended_at").timestamp(),
        back_at.timestamp(),
        "the gap ends when the host published again, not when somebody looked"
    );
    assert_eq!(
        record["duration_seconds"].as_i64(),
        Some(back_at.timestamp() - aged_at.timestamp()),
        "the duration is the outage this test staged"
    );
    assert_eq!(record["observed_by"], json!([READER]));
    assert_eq!(
        report["silences"][0]["duration_seconds"],
        record["duration_seconds"]
    );
}
