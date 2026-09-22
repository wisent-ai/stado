//! Why a candidate is refused, each refusal naming the host it is about.

use crate::hosts::{pressured_mini, roomy_laptop};
use crate::support::{
    fleet, memory, publish, relief, row, stderr, stdout, stado, verdict, FRESH_SECONDS, LAPTOP,
    LAPTOP_TOTAL_GB, MINI, PROFILE, RTX, STALE_SECONDS,
};

#[test]
fn a_candidate_under_pressure_itself_is_refused_by_name() {
    let store = fleet(MINI);
    publish(store.path(), MINI, FRESH_SECONDS, pressured_mini());
    publish(
        store.path(),
        LAPTOP,
        FRESH_SECONDS,
        memory(MINI_AVAILABLE_GB, LAPTOP_TOTAL_GB, MINI_SWAP_PCT, true),
    );

    let row = row(&relief(store.path()));
    assert_eq!(
        row["classification"], "no_destination_with_headroom",
        "{row}"
    );
    assert_eq!(verdict(&row, LAPTOP), "pressured");
    assert!(row["destination"].is_null(), "{row}");
}

#[test]
fn a_candidate_with_no_more_headroom_than_the_source_is_refused_by_name() {
    let store = fleet(MINI);
    publish(store.path(), MINI, FRESH_SECONDS, pressured_mini());
    publish(
        store.path(),
        LAPTOP,
        FRESH_SECONDS,
        memory(MINI_AVAILABLE_GB, LAPTOP_TOTAL_GB, 0.0, false),
    );

    let row = row(&relief(store.path()));
    assert_eq!(
        row["classification"], "no_destination_with_headroom",
        "{row}"
    );
    assert_eq!(verdict(&row, LAPTOP), "no_more_headroom_than_source");
}

#[test]
fn a_candidate_whose_publication_is_stale_is_refused_by_name() {
    let store = fleet(MINI);
    publish(store.path(), MINI, FRESH_SECONDS, pressured_mini());
    publish(store.path(), LAPTOP, STALE_SECONDS, roomy_laptop());

    let row = row(&relief(store.path()));
    assert_eq!(
        row["classification"], "no_destination_with_headroom",
        "{row}"
    );
    assert_eq!(verdict(&row, LAPTOP), "stale");
}

#[test]
fn a_candidate_that_never_published_is_refused_by_name() {
    let store = fleet(MINI);
    publish(store.path(), MINI, FRESH_SECONDS, pressured_mini());

    let row = row(&relief(store.path()));
    assert_eq!(
        row["classification"], "no_destination_with_headroom",
        "{row}"
    );
    assert_eq!(verdict(&row, LAPTOP), "no_publication");
}

/// The declared laptop is as short as the mini, but a registered Linux
/// workstation the profile does not declare has headroom: the plan names it
/// as the host to prepare, and the tick's standby pass is what would run.
#[test]
fn a_registered_host_with_headroom_is_planned_as_a_standby_when_no_declared_host_has_any() {
    let store = fleet(MINI);
    publish(store.path(), MINI, FRESH_SECONDS, pressured_mini());
    publish(
        store.path(),
        LAPTOP,
        FRESH_SECONDS,
        memory(MINI_AVAILABLE_GB, LAPTOP_TOTAL_GB, 0.0, false),
    );
    publish(store.path(), RTX, FRESH_SECONDS, roomy_laptop());

    let row = row(&relief(store.path()));
    assert_eq!(row["destination"], RTX, "{row}");
    assert_eq!(verdict(&row, LAPTOP), "no_more_headroom_than_source");
    assert_eq!(verdict(&row, RTX), "eligible");
    let rtx = row["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .find(|candidate| candidate["host"] == RTX)
        .cloned()
        .unwrap();
    assert_eq!(rtx["declared"], false, "{row}");
    assert!(
        row["detail"]
            .as_str()
            .is_some_and(|detail| detail.contains("can be prepared to stand by")),
        "{row}"
    );
}

/// With no other host at all, declared or registered, the refusal says so.
#[test]
fn no_host_anywhere_with_headroom_is_refused_by_name() {
    let store = fleet(MINI);
    publish(store.path(), MINI, FRESH_SECONDS, pressured_mini());
    publish(
        store.path(),
        LAPTOP,
        FRESH_SECONDS,
        memory(MINI_AVAILABLE_GB, LAPTOP_TOTAL_GB, 0.0, false),
    );
    publish(
        store.path(),
        RTX,
        FRESH_SECONDS,
        memory(MINI_AVAILABLE_GB, LAPTOP_TOTAL_GB, 0.0, false),
    );

    let row = row(&relief(store.path()));
    assert_eq!(
        row["classification"], "no_destination_with_headroom",
        "{row}"
    );
    assert_eq!(verdict(&row, RTX), "no_more_headroom_than_source");
}

