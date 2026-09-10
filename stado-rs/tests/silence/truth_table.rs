//! The transition truth table: what a silence record says about a host,
//! decided without a store.
use super::*;

#[test]
fn the_transition_truth_table_needs_no_store() {
    let beacon = at("2026-08-19T18:29:00Z");

    assert!(!beacon_is_silent(
        Some(beacon),
        at("2026-08-19T18:33:59Z"),
        300
    ));
    assert!(beacon_is_silent(
        Some(beacon),
        at("2026-08-19T18:34:01Z"),
        300
    ));
    assert!(
        beacon_is_silent(None, at("2026-08-19T18:34:01Z"), 300),
        "a host that never published is not a host that is fine"
    );
    assert!(
        !beacon_is_silent(Some(at("2026-08-19T19:00:00Z")), beacon, 300),
        "a publisher with a fast clock is not an outage"
    );

    // A close stamped before the open reports zero, never negative time.
    let mut skewed = open_record(HOST, beacon, READER_CLI, None);
    assert!(close_record(&mut skewed, at("2026-08-19T18:28:00Z")));
    assert_eq!(skewed.duration_seconds, Some(0));
    assert!(
        !close_record(&mut skewed, at("2026-08-19T18:40:00Z")),
        "a closed record does not close twice"
    );

    let mut record = open_record(HOST, beacon, READER_RESOLVER, None);
    assert!(merge_observation(&mut record, READER_CLI, Some("first")));
    assert!(!merge_observation(&mut record, READER_CLI, Some("second")));
    assert_eq!(
        record.first_reader_error.as_deref(),
        Some("first"),
        "the field records who noticed first, not who ran last"
    );
    assert_eq!(record.observed_by, vec!["resolver", "cli"]);
}
