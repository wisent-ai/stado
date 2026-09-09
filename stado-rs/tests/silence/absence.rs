//! What a real absence on this machine produces, read back through
//! `stado host link`.
//!
//! The incident, replayed against this host instead of narrated about
//! another: a beacon stops arriving, some reader notices, and afterwards
//! nothing in the product could say the gap had happened. Each case here
//! makes the absence real — a beacon object that is not there, or one whose
//! own instant is already past the threshold — runs the command an operator
//! runs during an outage, and asserts the record it left in the store and the
//! sentences it printed.

use chrono::{Duration, Utc};
use serde_json::json;

use crate::fixture::{Fleet, SILENCE_PREFIX, TARGET};
use crate::report::{
    blocker_saying, field_instant, only_silence, report, silences, stderr, stdout,
};

#[test]
fn a_host_that_published_nothing_is_recorded_once_with_the_readers_own_sentence() {
    let fleet = Fleet::local();
    let before = Utc::now();
    let out = fleet.link(TARGET);
    let row = report(&out);
    let after = Utc::now();

    // This machine answers its own channel while nothing has heard from its
    // beacon publisher. That is a box running and not reporting, which is a
    // different repair from a box that is gone — and the reason the verdict
    // is `degraded` rather than `silent`.
    assert_eq!(row["ssh_reachable"], json!(true), "{row}");
    assert_eq!(row["verdict"], "degraded", "{row}");
    assert!(
        row["beacon_age_seconds"].is_null(),
        "an absent beacon was given an age: {row}"
    );
    assert!(
        !out.status.success(),
        "a host nothing has heard from exited zero:\n{}",
        stdout(&out)
    );

    let record = only_silence(&row).clone();
    assert!(record["ended_at"].is_null(), "{record}");
    assert_eq!(record["observed_by"], json!(["cli"]), "{record}");
    // A host that never published starts its silence at the observation, not
    // at the epoch: the product may not report an outage nobody lived through
    // just because it has no earlier evidence.
    let started = field_instant(&record["started_at"]);
    assert!(
        started >= before && started <= after,
        "the gap was dated {started}, outside the run window {before}..{after}"
    );

    // The reader's own sentence, kept verbatim. Once the host is back this is
    // the only account of the gap that exists, so it has to name what was
    // looked for and it has to reach the operator.
    let first = record["first_reader_error"]
        .as_str()
        .unwrap_or_else(|| panic!("the record kept no reader error: {record}"));
    assert!(
        first.contains(&format!("no host health beacon for {TARGET:?}"))
            && first.contains(&format!("host_health/{TARGET}.json")),
        "the reader's sentence names neither the host nor the beacon path: {first}"
    );
    assert!(
        blocker_saying(&row, first),
        "the sentence in the record never reached the operator: {row}"
    );

    // The record is a blob under the canonical `state/` root, which is where
    // the object API authorizes these writes.
    let names = fleet.blobs(SILENCE_PREFIX, TARGET);
    assert_eq!(names.len(), 1, "{names:?}");
    assert_eq!(
        fleet.on_disk(SILENCE_PREFIX, TARGET, &names[0])["started_at"],
        record["started_at"]
    );

    // A second look by the same reader joins the open gap instead of opening
    // a rival one: the count an operator reads must measure the outage, not
    // how often somebody ran the diagnostic.
    let again = report(&fleet.link(TARGET));
    assert_eq!(only_silence(&again)["started_at"], record["started_at"]);
    assert_eq!(
        fleet.blobs(SILENCE_PREFIX, TARGET),
        names,
        "the second look opened a second record"
    );
}

#[test]
fn the_gap_starts_at_the_last_beacon_and_ends_at_the_fresher_one() {
    let fleet = Fleet::local();
    // Ten minutes since this host was last heard from, which is past the
    // declared 300s threshold.
    let last = fleet.publish_beacon(Utc::now() - Duration::seconds(600));
    let opening = fleet.link(TARGET);
    let opened = report(&opening);

    let age = opened["beacon_age_seconds"]
        .as_i64()
        .unwrap_or_else(|| panic!("the report gave the beacon no age: {opened}"));
    assert!(
        (600..=630).contains(&age),
        "a 600s-old beacon was aged {age}s: {opened}"
    );
    assert!(
        blocker_saying(&opened, "past the 300s silence threshold"),
        "the crossing was not named to the operator: {opened}"
    );
    assert!(!opening.status.success());

    let record = only_silence(&opened).clone();
    assert_eq!(
        field_instant(&record["started_at"]),
        last,
        "the gap is keyed by when the host was last heard from, not by when \
         somebody noticed: {record}"
    );

    // The host publishes again. That beacon, and not the moment anybody
    // looked, is when the gap ended — otherwise the duration measures the
    // polling interval instead of the outage.
    let back = fleet.publish_beacon(Utc::now());
    let closing = fleet.link(TARGET);
    let closed = report(&closing);
    assert_eq!(closed["verdict"], "healthy", "{closed}");
    assert!(
        closing.status.success(),
        "a repaired host exited non-zero:\n{}",
        stderr(&closing)
    );

    let record = only_silence(&closed).clone();
    assert_eq!(field_instant(&record["ended_at"]), back, "{record}");
    assert_eq!(
        record["duration_seconds"],
        json!(back.signed_duration_since(last).num_seconds()),
        "the reported duration is not the beacon-to-beacon gap: {record}"
    );
    assert_eq!(
        fleet.blobs(SILENCE_PREFIX, TARGET).len(),
        1,
        "the whole outage is one record"
    );

    // A closed gap is not reopened, and not closed a second time at a later
    // beacon's instant.
    fleet.publish_beacon(Utc::now());
    let after = report(&fleet.link(TARGET));
    assert_eq!(only_silence(&after)["ended_at"], record["ended_at"]);
}

#[test]
fn a_beacon_stamped_in_the_future_is_not_an_outage() {
    let fleet = Fleet::local();
    // A publisher whose clock runs an hour fast. Reporting that as an outage
    // sends an operator to the wrong machine.
    fleet.publish_beacon(Utc::now() + Duration::hours(1));
    let out = fleet.link(TARGET);
    let row = report(&out);

    assert!(
        row["beacon_age_seconds"]
            .as_i64()
            .is_some_and(|age| age < 0),
        "{row}"
    );
    assert_eq!(row["verdict"], "healthy", "{row}");
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(
        silences(&row).is_empty(),
        "clock skew on the publisher was recorded as a silence: {row}"
    );
    assert!(fleet.blobs(SILENCE_PREFIX, TARGET).is_empty());
}

#[test]
fn a_beacon_older_than_the_gap_it_closes_reports_zero_rather_than_negative_time() {
    let fleet = Fleet::local();
    // Absence opens the gap at the observation.
    let opened = report(&fleet.link(TARGET));
    let started = field_instant(&only_silence(&opened)["started_at"]);

    // The host is back and the two clocks disagree: the beacon that proves it
    // is back is stamped a minute before the gap was opened. The fresher
    // beacon still closes the gap, because it is the proof the host returned.
    let back = fleet.publish_beacon(started - Duration::seconds(60));
    let out = fleet.link(TARGET);
    let closed = report(&out);
    let record = only_silence(&closed).clone();

    assert_eq!(field_instant(&record["ended_at"]), back, "{record}");
    assert_eq!(
        record["duration_seconds"],
        json!(0),
        "a disagreement between two clocks was reported as negative time: {record}"
    );
    assert_eq!(closed["verdict"], "healthy", "{closed}");
    assert!(out.status.success(), "{}", stderr(&out));
}

#[test]
fn the_operators_threshold_decides_when_quiet_becomes_a_silence() {
    // Two minutes of quiet, which the fleet's one-minute publication timer
    // makes ordinary: inside the declared 300s threshold nothing is recorded,
    // because one missed publication is not an outage.
    let tolerated = Fleet::local();
    tolerated.publish_beacon(Utc::now() - Duration::seconds(120));
    let out = tolerated.link(TARGET);
    let row = report(&out);
    assert_eq!(row["verdict"], "healthy", "{row}");
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(silences(&row).is_empty(), "{row}");

    // The same two minutes, on a fleet whose operator declared 45 seconds:
    // now it is an outage, and the sentence names the threshold that decided.
    let tightened = Fleet::local();
    let last = tightened.publish_beacon(Utc::now() - Duration::seconds(120));
    let out = tightened.link_with_threshold(TARGET, "45");
    let row = report(&out);
    assert!(!out.status.success());
    assert!(
        blocker_saying(&row, "past the 45s silence threshold"),
        "the override never reached the sentence: {row}"
    );
    assert_eq!(
        field_instant(&only_silence(&row)["started_at"]),
        last,
        "{row}"
    );

    // A typo in the override must not switch the detector off: ten minutes of
    // quiet is still an outage, judged at the declared default.
    let mistyped = Fleet::local();
    mistyped.publish_beacon(Utc::now() - Duration::seconds(600));
    let out = mistyped.link_with_threshold(TARGET, "not a number");
    let row = report(&out);
    assert!(!out.status.success());
    assert!(
        blocker_saying(&row, "past the 300s silence threshold"),
        "a typo moved the threshold: {row}"
    );
    assert_eq!(mistyped.blobs(SILENCE_PREFIX, TARGET).len(), 1, "{row}");
}
