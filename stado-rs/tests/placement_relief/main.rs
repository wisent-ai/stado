//! What placement relief decides, read through `stado placement relief`
//! against an isolated fleet: a placed host over its memory watermark with a
//! declared host that has more headroom is a planned move to that host; a
//! stale publication moves nothing; a pressured or smaller candidate is
//! refused by name; and a profile relocated within the cooldown stays.

mod standby;
mod support;

use support::{
    fleet, memory, publish, relief, row, stado, stderr, stdout, verdict, FRESH_SECONDS, LAPTOP,
    LAPTOP_AVAILABLE_GB, LAPTOP_TOTAL_GB, MINI, MINI_AVAILABLE_GB, MINI_SWAP_PCT, MINI_TOTAL_GB,
    PROFILE, RELIEF_SCHEMA_VERSION, RTX, STALE_SECONDS,
};

/// The mini's own publication when a dip has passed: memory back above its
/// floor, swap still high, and its agent calling the pressure clear — the
/// exact reading the tick sampled on 2026-09-21 and settled on.
const MINI_CLEAR_AVAILABLE_GB: f64 = 2.5;
/// An age inside the stage's 900-second pressure window, and one past it.
const INSIDE_PRESSURE_WINDOW_SECONDS: i64 = 120;
const PAST_PRESSURE_WINDOW_SECONDS: i64 = 1200;

fn pressured_mini() -> serde_json::Value {
    memory(MINI_AVAILABLE_GB, MINI_TOTAL_GB, MINI_SWAP_PCT, true)
}

fn clear_mini() -> serde_json::Value {
    memory(MINI_CLEAR_AVAILABLE_GB, MINI_TOTAL_GB, MINI_SWAP_PCT, false)
}

fn roomy_laptop() -> serde_json::Value {
    memory(LAPTOP_AVAILABLE_GB, LAPTOP_TOTAL_GB, 0.0, false)
}

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

#[test]
fn a_profile_relocated_within_the_cooldown_stays_where_it_landed() {
    let store = fleet(MINI);
    publish(store.path(), MINI, FRESH_SECONDS, pressured_mini());
    publish(store.path(), LAPTOP, FRESH_SECONDS, roomy_laptop());
    support::write(
        store.path(),
        "state/autonomy/placement_relief/latest.json",
        &serde_json::json!({
            "schema_version": RELIEF_SCHEMA_VERSION,
            "decision_id": "placement-relief-previous",
            "created_at": support::ago(FRESH_SECONDS),
            "mode": "enforce-safe",
            "summary": {},
            "rows": [],
            "relocations": { PROFILE: support::ago(FRESH_SECONDS) }
        }),
    );

    let report = relief(store.path());
    let row = row(&report);
    assert_eq!(row["classification"], "moved_recently", "{row}");
    assert_eq!(
        row["destination"], LAPTOP,
        "the row still names where it would go: {row}"
    );
    assert_eq!(
        report["last_report"]["decision_id"], "placement-relief-previous",
        "{report}"
    );
}

/// The plain listing an operator reads at the terminal names the host, the
/// verdict and the memory behind it on one line each.
#[test]
fn the_plain_listing_names_the_move_and_every_candidate() {
    let store = fleet(MINI);
    publish(store.path(), MINI, FRESH_SECONDS, pressured_mini());
    publish(store.path(), LAPTOP, FRESH_SECONDS, roomy_laptop());

    let out = stado(store.path(), &["placement", "relief"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let text = stdout(&out);
    assert!(
        text.contains(&format!("{PROFILE}\t{MINI}\t\t{LAPTOP}\t")),
        "{text}"
    );
    assert!(
        text.contains(&format!("\t{LAPTOP}\tdeclared\teligible\t")),
        "{text}"
    );
}

/// A host that publishes clear this second and published pressure two
/// minutes ago is still pressured here.
///
/// charless-mac-mini declares a 2 GiB floor and crosses it every few minutes.
/// On 2026-09-21 the tick at 18:01:19Z sampled it at `2.5 GiB available,
/// pressure clear`, wrote `settled`, and moved nothing, while every reading
/// taken by hand that hour — including one seconds later — saw `1.7 GiB
/// available, pressure active`. One sample of an oscillating host is not
/// evidence that it is healthy, so pressure sticks for the declared window.
#[test]
fn a_host_that_published_pressure_inside_the_window_is_still_pressured() {
    let store = fleet(MINI);
    publish(store.path(), MINI, FRESH_SECONDS, clear_mini());
    publish(store.path(), LAPTOP, FRESH_SECONDS, roomy_laptop());
    support::write(
        store.path(),
        "state/autonomy/placement_relief/latest.json",
        &serde_json::json!({
            "schema_version": RELIEF_SCHEMA_VERSION,
            "decision_id": "placement-relief-previous",
            "created_at": support::ago(INSIDE_PRESSURE_WINDOW_SECONDS),
            "mode": "enforce-safe",
            "summary": {},
            "rows": [],
            "relocations": {},
            "pressure_seen": { MINI: support::ago(INSIDE_PRESSURE_WINDOW_SECONDS) }
        }),
    );

    let row = row(&relief(store.path()));
    assert_eq!(
        row["destination"], LAPTOP,
        "a host that dipped below its watermark inside the window settled: {row}"
    );
    assert_eq!(
        row["classification"], "",
        "the move is due, so the row carries no classification until the tick gates it: {row}"
    );
}

/// Once the window has passed with nothing but clear publications, the
/// profile settles: the stage holds a host pressured, it does not condemn it.
#[test]
fn a_host_clear_for_the_whole_window_settles() {
    let store = fleet(MINI);
    publish(store.path(), MINI, FRESH_SECONDS, clear_mini());
    publish(store.path(), LAPTOP, FRESH_SECONDS, roomy_laptop());
    support::write(
        store.path(),
        "state/autonomy/placement_relief/latest.json",
        &serde_json::json!({
            "schema_version": RELIEF_SCHEMA_VERSION,
            "decision_id": "placement-relief-previous",
            "created_at": support::ago(PAST_PRESSURE_WINDOW_SECONDS),
            "mode": "enforce-safe",
            "summary": {},
            "rows": [],
            "relocations": {},
            "pressure_seen": { MINI: support::ago(PAST_PRESSURE_WINDOW_SECONDS) }
        }),
    );

    let row = row(&relief(store.path()));
    assert_eq!(row["classification"], "settled", "{row}");
    assert_eq!(row["destination"], serde_json::Value::Null, "{row}");
}

/// A destination that published pressure inside the same window is refused,
/// for the same reason the source is held: its clear reading is one sample of
/// a host that keeps crossing its floor.
#[test]
fn a_candidate_pressured_inside_the_window_is_refused() {
    let store = fleet(MINI);
    publish(store.path(), MINI, FRESH_SECONDS, pressured_mini());
    publish(store.path(), LAPTOP, FRESH_SECONDS, roomy_laptop());
    support::write(
        store.path(),
        "state/autonomy/placement_relief/latest.json",
        &serde_json::json!({
            "schema_version": RELIEF_SCHEMA_VERSION,
            "decision_id": "placement-relief-previous",
            "created_at": support::ago(INSIDE_PRESSURE_WINDOW_SECONDS),
            "mode": "enforce-safe",
            "summary": {},
            "rows": [],
            "relocations": {},
            "pressure_seen": { LAPTOP: support::ago(INSIDE_PRESSURE_WINDOW_SECONDS) }
        }),
    );

    let row = row(&relief(store.path()));
    assert_eq!(verdict(&row, LAPTOP), "pressured", "{row}");
    assert_eq!(
        row["classification"], "no_destination_with_headroom",
        "a host that was pressured minutes ago was used as headroom: {row}"
    );
}
