//! What the operator surface carries of a host's silence history.
//!
//! A host that drops off every afternoon has to read as a pattern, so the
//! report carries more than the current gap; and it has to stay readable on a
//! terminal during the outage it describes, so it carries five. The store
//! keeps everything either way: the cap trims the report, never the history.

use chrono::{Duration, Utc};

use crate::fixture::{Fleet, SILENCE_PREFIX, TARGET};
use crate::report::{field_instant, report, silences, stdout};

/// Gaps to seed, more than the report carries.
const SEEDED: i64 = 7;

#[test]
fn link_reports_the_newest_five_gaps_newest_first_and_keeps_the_rest_in_the_store() {
    let fleet = Fleet::local();
    let now = Utc::now();

    // Seven closed gaps through the afternoon, seven minutes each, written in
    // the product's own record shape under the keys it keys them by.
    let seeded: Vec<_> = (1..=SEEDED)
        .map(|hours| {
            let started = now - Duration::hours(hours) - Duration::seconds(420);
            fleet.seed_closed_silence(started, started + Duration::seconds(420));
            started
        })
        .collect();

    // No beacon in this store, so the run also opens the current gap: what an
    // operator sees during an outage is this gap on top of the recent ones.
    let out = fleet.link(TARGET);
    let row = report(&out);
    let reported = silences(&row);

    assert_eq!(
        reported.len(),
        5,
        "the report carried {} gaps: {row}",
        reported.len()
    );
    assert!(
        reported[0]["ended_at"].is_null(),
        "the newest reported gap is not the open one: {row}"
    );
    let instants: Vec<_> = reported
        .iter()
        .map(|record| field_instant(&record["started_at"]))
        .collect();
    assert!(
        instants.windows(2).all(|pair| pair[0] > pair[1]),
        "the gaps are not newest first: {instants:?}"
    );

    // The report holds the open gap plus the four newest closed ones, so the
    // three oldest are outside it and still in the store: an operator asking
    // a longer question than the report answers has the records to answer it
    // with.
    let oldest = field_instant(&reported[4]["started_at"]);
    let trimmed = seeded.iter().filter(|started| **started < oldest).count();
    assert_eq!(
        trimmed,
        usize::try_from(SEEDED).expect("seven fits") - 4,
        "the report did not trim exactly the oldest gaps: {instants:?}"
    );
    assert_eq!(
        fleet.blobs(SILENCE_PREFIX, TARGET).len(),
        usize::try_from(SEEDED).expect("seven fits") + 1,
        "the report's cap trimmed the store"
    );
    assert!(!out.status.success());
}

#[test]
fn the_terminal_rendering_names_each_gap_and_what_the_reader_saw() {
    let fleet = Fleet::local();
    let started = Utc::now() - Duration::hours(1) - Duration::seconds(420);
    fleet.seed_closed_silence(started, started + Duration::seconds(420));

    // Without --json: the shape an operator reads during the outage.
    let out = fleet.stado(&["host", "link", TARGET]);
    let printed = stdout(&out);

    assert!(
        printed.contains("silences: 2 recorded, newest first"),
        "the terminal rendering did not count the gaps:\n{printed}"
    );
    assert!(
        printed.contains("420s"),
        "a closed gap's duration is not printed:\n{printed}"
    );
    assert!(
        printed.contains("still open"),
        "the gap this run opened is not printed as open:\n{printed}"
    );
    assert!(
        printed.contains("first reader error: no host health beacon"),
        "the reader's own sentence is not printed:\n{printed}"
    );
    assert!(
        printed.contains("refusals: none in the last 3600s"),
        "the refusal window is not reported when nothing refused:\n{printed}"
    );
}
