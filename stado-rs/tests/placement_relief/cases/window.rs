//! What the cooldown, the plain listing and the sticky pressure window do.

use crate::hosts::{clear_mini, pressured_mini, roomy_laptop};
use crate::hosts::{INSIDE_PRESSURE_WINDOW_SECONDS, PAST_PRESSURE_WINDOW_SECONDS};
use crate::support::{
    self, fleet, publish, relief, row, stado, stderr, stdout, verdict, FRESH_SECONDS, LAPTOP, MINI,
    PROFILE, RELIEF_SCHEMA_VERSION,
};

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
/// A 16 GiB always-on Mac declares a 2 GiB floor and crosses it every few
/// minutes. On 2026-09-21 the tick sampled one at `2.5 GiB available,
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
