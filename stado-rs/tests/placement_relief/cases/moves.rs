//! When a move is planned, and when the host is left where it is.

use crate::hosts::{pressured_mini, roomy_laptop};
use crate::support::memory;
use crate::support::{
    fleet, publish, relief, row, verdict, FRESH_SECONDS, LAPTOP, LAPTOP_AVAILABLE_GB, MINI,
    MINI_TOTAL_GB, STALE_SECONDS,
};

#[test]
fn a_pressured_host_with_a_roomier_declared_host_plans_the_move() {
    let store = fleet(MINI);
    publish(store.path(), MINI, FRESH_SECONDS, pressured_mini());
    publish(store.path(), LAPTOP, FRESH_SECONDS, roomy_laptop());

    let report = relief(store.path());
    let row = row(&report);
    assert_eq!(row["placed_on"], MINI, "{row}");
    assert_eq!(row["destination"], LAPTOP, "{row}");
    assert_eq!(verdict(&row, LAPTOP), "eligible");
    assert_eq!(
        row["classification"], "",
        "a due move carries no classification until the tick gates it: {row}"
    );
    assert!(
        row["detail"]
            .as_str()
            .is_some_and(|detail| detail.contains("over its watermark")),
        "{row}"
    );
    assert_eq!(report["hosts"][MINI]["pressure_active"], true, "{report}");
    assert_eq!(
        report["hosts"][LAPTOP]["pressure_active"], false,
        "{report}"
    );
}

#[test]
fn a_host_under_its_watermark_is_settled() {
    let store = fleet(MINI);
    publish(
        store.path(),
        MINI,
        FRESH_SECONDS,
        memory(LAPTOP_AVAILABLE_GB, MINI_TOTAL_GB, 0.0, false),
    );
    publish(store.path(), LAPTOP, FRESH_SECONDS, roomy_laptop());

    let row = row(&relief(store.path()));
    assert_eq!(row["classification"], "settled", "{row}");
    assert!(row["destination"].is_null(), "{row}");
    assert!(
        row["candidates"].as_array().is_some_and(Vec::is_empty),
        "{row}"
    );
}

#[test]
fn a_stale_publication_from_the_placed_host_moves_nothing() {
    let store = fleet(MINI);
    publish(store.path(), MINI, STALE_SECONDS, pressured_mini());
    publish(store.path(), LAPTOP, FRESH_SECONDS, roomy_laptop());

    let row = row(&relief(store.path()));
    assert_eq!(row["classification"], "evidence_stale", "{row}");
    assert!(row["destination"].is_null(), "{row}");
}

#[test]
fn a_placed_host_that_never_published_moves_nothing() {
    let store = fleet(MINI);
    publish(store.path(), LAPTOP, FRESH_SECONDS, roomy_laptop());

    let row = row(&relief(store.path()));
    assert_eq!(row["classification"], "evidence_stale", "{row}");
    assert!(
        row["detail"]
            .as_str()
            .is_some_and(|detail| detail.contains("published no capacity")),
        "{row}"
    );
}
