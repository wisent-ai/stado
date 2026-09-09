//! The readers' half of the incident: what refused while a host said nothing.
//!
//! During the outage two readers did notice, and both wrote their refusals to
//! `~/.stado/logs/stado-resolver.err` — true, timestamped, and read by nobody.
//! A refusal only a log file knows about is a refusal the product did not
//! make. So a failed read publishes a record about the host it is evidence
//! about, and `stado host link` on that host reports it.
//!
//! The absence here is real and local: the authority's declared ssh
//! destination is under `.invalid`, the TLD RFC 2606 reserves, so the name
//! resolution genuinely fails on this machine and no packet leaves it.

use chrono::{Duration, Utc};
use serde_json::json;

use crate::fixture::{Fleet, AUTHORITY, REFUSAL_PREFIX};
use crate::report::{blocker_saying, report, stderr, stdout};

/// The read that goes through the service-directory authority.
const RESOLVE: [&str; 5] = ["resolver", "resolve", "brama", "--consumer", "lem"];

#[test]
fn an_unreachable_authority_publishes_its_own_sentence_as_a_refusal() {
    let fleet = Fleet::with_unreachable_authority();
    let out = fleet.stado(&RESOLVE);

    assert!(
        !out.status.success(),
        "an unreachable authority resolved: {}",
        stdout(&out)
    );
    let printed = stderr(&out);
    assert!(
        printed.contains("registry authority"),
        "the command did not reach the authority read: {printed}"
    );

    let names = fleet.blobs(REFUSAL_PREFIX, AUTHORITY);
    assert_eq!(
        names.len(),
        1,
        "the failed read published no refusal (stderr: {printed})"
    );
    let record = fleet.on_disk(REFUSAL_PREFIX, AUTHORITY, &names[0]);
    assert_eq!(
        record["host"], AUTHORITY,
        "the refusal is filed under the host it is evidence about, not the \
         machine that noticed"
    );
    assert_eq!(record["reader"], "cli", "{record}");
    assert_eq!(record["reason"], "authority_unreachable", "{record}");
    let detail = record["detail"].as_str().expect("detail is a string");
    assert!(
        printed.contains(detail),
        "the stored detail is not the sentence the command printed:\n  stored: {detail}\n  printed: {printed}"
    );
}

#[test]
fn link_calls_the_unreachable_host_silent_and_counts_what_refused_because_of_it() {
    let fleet = Fleet::with_unreachable_authority();
    let refused = fleet.stado(&RESOLVE);
    assert!(!refused.status.success());

    let out = fleet.link(AUTHORITY);
    let row = report(&out);

    // Nothing answered its channel and nothing has heard from its beacon:
    // that host is silent, not merely degraded.
    assert_eq!(row["ssh_reachable"], json!(false), "{row}");
    assert_eq!(row["verdict"], "silent", "{row}");
    assert!(
        !out.status.success(),
        "a silent host exited zero:\n{}",
        stdout(&out)
    );

    // The refusal the failed read published, counted per reason inside the
    // command's own window — the consequence beside the cause, which is the
    // half that used to live only in a log file.
    assert_eq!(
        row["reader_refusals"],
        json!({
            "window_seconds": 3600,
            "count": 1,
            "reasons": {"authority_unreachable": 1},
        }),
        "{row}"
    );
    assert!(
        blocker_saying(
            &row,
            "readers refused 1 time(s) in the last 3600s: authority_unreachable=1"
        ),
        "the refusal never reached the operator: {row}"
    );
}

#[test]
fn a_refusal_older_than_the_window_stays_in_the_store_and_stops_being_counted() {
    let fleet = Fleet::with_unreachable_authority();
    assert!(!fleet.stado(&RESOLVE).status.success());

    // The product's own refusal document, moved two hours back: the same
    // record, a different instant. An hour is the span an operator asking
    // "why did this host go quiet" has in mind.
    fleet.redate_refusal(AUTHORITY, Utc::now() - Duration::hours(2));

    let row = report(&fleet.link(AUTHORITY));
    assert_eq!(
        row["reader_refusals"]["count"],
        json!(0),
        "a two-hour-old refusal was counted inside the one-hour window: {row}"
    );
    assert!(
        !blocker_saying(&row, "readers refused"),
        "an out-of-window refusal was reported as current: {row}"
    );
    assert_eq!(
        fleet.blobs(REFUSAL_PREFIX, AUTHORITY).len(),
        1,
        "the record left the store when it left the window"
    );
}
